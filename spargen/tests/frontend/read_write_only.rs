//! `readOnly`/`writeOnly` properties: `required` applies to them in one direction only, so the one
//! generated struct relaxes it (`W015`) — a `writeOnly` member decodes as absent, and a `readOnly`
//! one is never serialized and is not a `new` argument (#1326).

use super::*;

/// The `User` schema both directions share, used as a request body and as a response.
const USER: &str = r##"
openapi: 3.1.0
info: { title: T, version: 1.0.0 }
paths:
  /users:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/User" }
      responses:
        "201":
          description: Created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/User" }
components:
  schemas:
    User:
      type: object
      required: [id, name, password]
      properties:
        id: { type: integer, readOnly: true }
        name: { type: string }
        password: { type: string, writeOnly: true }
        nickname: { type: string, readOnly: true }
"##;

/// The `W015` messages of `report`, in emission order.
fn relaxed(report: &Report) -> Vec<String> {
    report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == Code::ReadWriteOnlyRequirednessRelaxed)
        .map(|diagnostic| diagnostic.message.clone())
        .collect()
}

/// `code` with every run of whitespace collapsed to one space, so a multi-line attribute or
/// signature reads as one line.
fn flat(code: &str) -> String {
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Assert the whitespace-collapsed `code` carries each of `snippets`.
fn assert_carries(code: &str, snippets: &[&str]) {
    let flat = flat(code);
    for snippet in snippets {
        assert!(flat.contains(snippet), "missing `{snippet}`: {code}");
    }
}

#[test]
fn a_required_read_or_write_only_property_is_relaxed_and_warned() {
    let (report, code) = generate_with_code(USER);
    assert_eq!(report.outcome(), Outcome::Generated, "{report:#?}");
    // One warning per relaxed `required` entry, and none for the optional `readOnly` `nickname`.
    assert_eq!(
        relaxed(&report),
        [
            "property `id` of `User` is `required` and `readOnly`: a request leaves it out, so its \
             member may be absent, is never serialized, and is not a `new` argument",
            "property `password` of `User` is `required` and `writeOnly`: a response leaves it \
             out, so its member may be absent when decoded, and `new` still takes it",
        ],
        "{report:#?}"
    );
    assert_carries(
        &code,
        &[
            // Both relaxed members may be absent when decoded. A `readOnly` member is never
            // serialized, required or not; a `writeOnly` one is left off only when absent.
            "#[serde( rename = \"id\", default, skip_serializing, deserialize_with = \"decode_present\", )] pub id: Option<UserId>,",
            "#[serde(rename = \"name\")] pub name: UserName,",
            "#[serde( rename = \"password\", default, skip_serializing_if = \"Option::is_none\", deserialize_with = \"decode_present\", )] pub password: Option<UserPassword>,",
            "#[serde( rename = \"nickname\", default, skip_serializing, deserialize_with = \"decode_present\", )] pub nickname: Option<UserNickname>,",
            // `new` takes the `writeOnly` member every request carries, not the `readOnly` one,
            // which a setter sets as it does an optional member.
            "pub fn new(name: UserName, password: UserPassword) -> Self { Self { id: None, name, password: Some(password), nickname: None, } }",
            "pub fn with_id(mut self, value: UserId) -> Self {",
        ],
    );
    assert!(!flat(&code).contains("pub fn with_password("), "{code}");
}

/// The relaxation reads the merged struct: a property one `allOf` member marks `readOnly` and
/// another requires is relaxed and warned once.
#[test]
fn a_property_an_all_of_member_requires_is_relaxed_on_the_merged_struct() {
    let spec = with_schemas(
        "3.1.0",
        "    Base:\n      type: object\n      properties:\n        id: { type: integer, readOnly: true }\n    \
         Item:\n      allOf:\n        - $ref: '#/components/schemas/Base'\n        - type: object\n          \
         required: [id]\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    let warned = relaxed(&report);
    assert_eq!(warned.len(), 1, "{report:#?}");
    assert!(warned[0].contains("property `id` of `Item`"), "{warned:?}");
    assert_carries(
        &code,
        &["impl Item { ///A `Item` with every optional member"],
    );
}

/// A nullable `required` `writeOnly` member decodes as a `Nullable` and `new` takes the `Option`
/// a required nullable member is, its `None` the explicit `Null` (#1414: spelled out, since
/// `Nullable` has no `From<Option<T>>`).
#[test]
fn a_nullable_required_write_only_property_is_a_nullable_member() {
    let spec = with_schemas(
        "3.1.0",
        "    Secret:\n      type: object\n      required: [token]\n      properties:\n        \
         token: { type: [string, 'null'], writeOnly: true }\n",
    );
    let (report, code) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert_eq!(relaxed(&report).len(), 1, "{report:#?}");
    assert_carries(
        &code,
        &[
            "pub token: super::support::Nullable<SecretToken>,",
            "pub fn new(token: Option<SecretToken>) -> Self { Self { token: match token { \
             Some(value) => super::support::Nullable::Value(value), None => \
             super::support::Nullable::Null, }, } }",
        ],
    );
}

/// An optional `readOnly` or `writeOnly` property relaxes nothing, so nothing is warned.
#[test]
fn an_optional_read_or_write_only_property_is_not_warned() {
    let spec = with_schemas(
        "3.1.0",
        "    Item:\n      type: object\n      properties:\n        id: { type: integer, readOnly: true }\n        \
         secret: { type: string, writeOnly: true }\n",
    );
    let (report, _) = generate_with_code(&spec);
    assert_ne!(report.outcome(), Outcome::Rejected, "{report:#?}");
    assert!(relaxed(&report).is_empty(), "{report:#?}");
}

/// Where both `allOf` members declare the property, the merge is `readOnly` whichever member says
/// so: the requirement is relaxed and warned once in either order of the members.
#[test]
fn a_property_both_all_of_members_declare_is_relaxed_in_either_order() {
    let required = "- type: object\n          required: [id]\n          properties:\n            \
                    id: { type: integer }";
    let read_only = "- type: object\n          properties:\n            \
                     id: { type: integer, readOnly: true }";
    for (order, first, second) in [
        ("required first", required, read_only),
        ("readOnly first", read_only, required),
    ] {
        let spec = with_schemas(
            "3.1.0",
            &format!("    Item:\n      allOf:\n        {first}\n        {second}\n"),
        );
        let (report, code) = generate_with_code(&spec);
        assert_ne!(report.outcome(), Outcome::Rejected, "{order}: {report:#?}");
        let warned = relaxed(&report);
        assert_eq!(warned.len(), 1, "{order}: {report:#?}");
        assert!(
            warned[0].contains("property `id` of `Item`"),
            "{order}: {warned:?}"
        );
        assert_carries(
            &code,
            &["#[serde( rename = \"id\", default, skip_serializing, deserialize_with = \"decode_present\", )] pub id: Option<"],
        );
    }
}
