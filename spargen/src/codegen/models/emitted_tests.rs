use crate::codegen::test_harness::{assert_contains_all, between, generated_with};

/// One definition per model shape the `types` module emits: string enums (`Color`), checked
/// integer and boolean sets, one documented (`Level`, `Flag`), a base64 newtype (`Blob`), structs
/// renamed for XML or not (`Root`, `Plain`), a struct with every kind of member (`Thing`), structs
/// whose `Default` is derived, with defaulted members (`Defaulted`) or without (`Loose`), or
/// absent (`Only`), aliases (`Pair`, `Anything`, `Nil`), an uninhabited definition (`Nothing`) and
/// a newtype closing an alias cycle (`Cycle`).
const MODELS: &str = r##"openapi: 3.1.0
info: {title: T, version: '1'}
components:
  schemas:
    Color: {type: string, enum: [red, green]}
    Level: {type: integer, enum: [-3, 0, 3], description: A level.}
    Flag: {type: boolean, enum: [true]}
    Blob: {type: string, contentEncoding: base64}
    Root: {type: object, xml: {name: root-el}, properties: {a: {type: string}}}
    Plain: {type: object, xml: {name: Plain}, properties: {a: {type: string}}}
    Thing:
      type: object
      required: [id, label, maybe, secret, hint]
      properties:
        id: {type: integer, readOnly: true}
        label: {type: string}
        maybe: {type: [string, 'null']}
        secret: {type: string, writeOnly: true}
        hint: {type: [string, 'null'], writeOnly: true}
        both: {type: string, readOnly: true, writeOnly: true}
        note: {type: [string, 'null']}
        count: {type: integer, default: 5}
        color: {$ref: '#/components/schemas/Color', default: green}
        level: {$ref: '#/components/schemas/Level', default: 3}
        old: {type: string, deprecated: true}
      additionalProperties: {type: integer}
    Loose: {type: object, properties: {x: {type: string}}}
    Defaulted: {type: object, properties: {n: {type: number, default: 1.5}, s: {type: string, default: hi}, b: {type: boolean, default: true}}}
    Only: {type: object, required: [a], properties: {a: {type: string}}}
    Pair: {type: array, prefixItems: [{type: string}, {type: integer}], items: false}
    Anything: {}
    Nil: {type: 'null'}
    Nothing: false
    Cycle: {type: array, items: {$ref: '#/components/schemas/Cycle'}}
    Base: {type: object, properties: {kind: {type: string}, code: {type: integer}}}
    Narrowed:
      allOf:
        - $ref: '#/components/schemas/Base'
        - properties: {kind: {const: special}, code: {const: 7}}
paths:
  /models:
    post:
      operationId: postModels
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                color: {$ref: '#/components/schemas/Color'}
                level: {$ref: '#/components/schemas/Level'}
                flag: {$ref: '#/components/schemas/Flag'}
                blob: {$ref: '#/components/schemas/Blob'}
                root: {$ref: '#/components/schemas/Root'}
                plain: {$ref: '#/components/schemas/Plain'}
                thing: {$ref: '#/components/schemas/Thing'}
                narrowed: {$ref: '#/components/schemas/Narrowed'}
                loose: {$ref: '#/components/schemas/Loose'}
                defaulted: {$ref: '#/components/schemas/Defaulted'}
                only: {$ref: '#/components/schemas/Only'}
                pair: {$ref: '#/components/schemas/Pair'}
                anything: {$ref: '#/components/schemas/Anything'}
                nil: {$ref: '#/components/schemas/Nil'}
                nothing: {$ref: '#/components/schemas/Nothing'}
                cycle: {$ref: '#/components/schemas/Cycle'}
      responses:
        '200':
          description: narrowed where `open_narrowing` opens it
          content:
            application/json:
              schema:
                allOf:
                  - $ref: '#/components/schemas/Base'
                  - properties: {kind: {const: rare}, code: {const: 9}}
"##;

/// The `types` module `MODELS` generates, with `open_narrowing` as `open` says and the `testing`
/// module as `testing` says.
fn types(open: bool, testing: bool) -> String {
    let generated = generated_with(MODELS, |spec| {
        spec.with_open_narrowing(crate::Narrowing::from_on(open))
            .with_testing(crate::TestingModule::from_on(testing))
    });
    between(&generated, "pubmodtypes{", "pubstructClient{").to_owned()
}

/// A closed string enum derives serde under each value's rename and lists its values in `ALL`,
/// `as_str` and `FromStr`; an open one, from a narrowing `open_narrowing` opens, holds any other
/// string in one more variant, written and read by hand.
#[test]
fn string_enums_list_their_values_closed_or_open() {
    assert_contains_all(
        &types(false, false),
        &[concat!(
            r#"#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,Serialize,Deserialize)]pubenumColor"#,
            r#"{#[serde(rename="red")]Red,#[serde(rename="green")]Green,}implColor{"#,
            r#"///Everyvariant,intheorderthedescriptionlistsitsvalues.pubconstALL:&'static[Self]=&[Color::Red,Color::Green];"#,
            r#"///Thewirevalue.pubconstfnas_str(self)->&'staticstr{matchself{Color::Red=>"red",Color::Green=>"green",}}}"#,
            r#"implstd::fmt::DisplayforColor{fnfmt(&self,f:&mutstd::fmt::Formatter<'_>)->std::fmt::Result{f.write_str(self.as_str())}}"#,
            r#"implstd::str::FromStrforColor{typeErr=super::support::UnknownVariant;"#,
            r#"///Parseawirevalue:exactlyoneofthelistedvalues,matchedbyteforbyte."#,
            r#"fnfrom_str(value:&str)->Result<Self,super::support::UnknownVariant>{matchvalue{"#,
            r#""red"=>Ok(Color::Red),"green"=>Ok(Color::Green),_=>Err(super::support::UnknownVariant::new("Color",value)),}}}"#,
        )],
    );
    assert_contains_all(
        &types(true, false),
        &[concat!(
            r#"pubenumResponseBodyMember1Kind{Rare,"#,
            r#"///Avaluethedescriptiondoesnotlisthere.Decodingproducesitonlyforastringnoothervariantnames."#,
            r#"Other(String),}implResponseBodyMember1Kind{///Thewirevalue.pubfnas_str(&self)->&str{matchself{"#,
            r#"ResponseBodyMember1Kind::Rare=>"rare",ResponseBodyMember1Kind::Other(value)=>value.as_str(),}}}"#,
            r#"implstd::fmt::DisplayforResponseBodyMember1Kind{fnfmt(&self,f:&mutstd::fmt::Formatter<'_>)->std::fmt::Result{f.write_str(self.as_str())}}"#,
            r#"implserde::SerializeforResponseBodyMember1Kind{fnserialize<S>(&self,serializer:S)->Result<S::Ok,S::Error>"#,
            r#"whereS:serde::Serializer,{serializer.serialize_str(self.as_str())}}"#,
            r#"impl<'de>serde::Deserialize<'de>forResponseBodyMember1Kind{fndeserialize<D>(deserializer:D)->Result<Self,D::Error>"#,
            r#"whereD:serde::Deserializer<'de>,{letvalue=<Stringasserde::Deserialize>::deserialize(deserializer)?;"#,
            r#"Ok(matchvalue.as_str(){"rare"=>ResponseBodyMember1Kind::Rare,_=>ResponseBodyMember1Kind::Other(value),},)}}"#,
        )],
    );
}

/// An integer or boolean set is a checked newtype: a named `const` per value (`V_MINUS_` before a
/// negative one's magnitude), `ALL`, `get`, conversions, and a `TryFrom` serde decodes through; a
/// documented one keeps its docs, a blank line, then the values it holds. An open set decodes any
/// value of its width.
#[test]
fn integer_and_boolean_sets_are_checked_newtypes() {
    let closed = types(false, false);
    assert_contains_all(
        &closed,
        &[
            concat!(
                r#"///Alevel.//////Holdsexactlyoneof:`-3`,`0`,`3`."#,
                r#"#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,PartialOrd,Ord)]pubstructLevel(i64);implLevel{"#,
                r#"///Thevalue`-3`.pubconstV_MINUS_3:Self=Self(-3);///Thevalue`0`.pubconstV0:Self=Self(0);"#,
                r#"///Thevalue`3`.pubconstV3:Self=Self(3);"#,
                r#"///Everylistedvalue,intheorderthedescriptionliststhem.pubconstALL:&'static[Self]=&[Self::V_MINUS_3,Self::V0,Self::V3];"#,
                r#"///Thevalue.pubconstfnget(self)->i64{self.0}}"#,
                r#"implstd::fmt::DisplayforLevel{fnfmt(&self,f:&mutstd::fmt::Formatter<'_>)->std::fmt::Result{std::fmt::Display::fmt(&self.0,f)}}"#,
                r#"implFrom<Level>fori64{fnfrom(value:Level)->Self{value.0}}"#,
                r#"implTryFrom<i64>forLevel{typeError=super::support::UnknownVariant;///Acceptexactlyoneofthelistedvalues."#,
                r#"fntry_from(value:i64)->Result<Self,super::support::UnknownVariant>{ifSelf::ALL.iter().any(|listed|listed.0==value)"#,
                r#"{Ok(Self(value))}else{Err(super::support::UnknownVariant::new("Level",value.to_string()))}}}"#,
                r#"implserde::SerializeforLevel{fnserialize<S>(&self,serializer:S)->Result<S::Ok,S::Error>whereS:serde::Serializer,"#,
                r#"{serde::Serialize::serialize(&self.0,serializer)}}impl<'de>serde::Deserialize<'de>forLevel{"#,
                r#"fndeserialize<D>(deserializer:D)->Result<Self,D::Error>whereD:serde::Deserializer<'de>,"#,
                r#"{letvalue=<i64asserde::Deserialize>::deserialize(deserializer)?;Self::try_from(value).map_err(serde::de::Error::custom)}}"#,
            ),
            concat!(
                r#"}///Holdsexactlyoneof:`true`.#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,PartialOrd,Ord)]pubstructFlag(bool);"#,
                r#"implFlag{///Thevalue`true`.pubconstTRUE:Self=Self(true);"#,
            ),
        ],
    );
    assert_contains_all(
        &types(true, false),
        &[
            r#"///Lists`9`.Decodingalsoacceptsanyothervalue;`TryFrom`acceptsonlythelistedones."#,
            concat!(
                r#"impl<'de>serde::Deserialize<'de>forResponseBodyMember1Code{fndeserialize<D>(deserializer:D)->Result<Self,D::Error>"#,
                r#"whereD:serde::Deserializer<'de>,{letvalue=<i64asserde::Deserialize>::deserialize(deserializer)?;Ok(Self(value))}}"#,
            ),
        ],
    );
}

/// A base64 string is a newtype over the bytes, encoded through the runtime's codec; an
/// uninhabited definition refuses every value both ways; a self-reaching array is a transparent
/// newtype; every other unnamed shape is an alias.
#[test]
fn bytes_never_cycles_and_aliases_take_their_own_shapes() {
    assert_contains_all(
        &types(false, false),
        &[
            concat!(
                r#"#[derive(Debug,Clone,PartialEq,Eq,Hash,Default)]pubstructBlob(pubbytes::Bytes);"#,
                r#"implstd::ops::DerefforBlob{typeTarget=bytes::Bytes;fnderef(&self)->&bytes::Bytes{&self.0}}"#,
                r#"implFrom<bytes::Bytes>forBlob{fnfrom(bytes:bytes::Bytes)->Self{Blob(bytes)}}"#,
                r#"implFrom<Blob>forbytes::Bytes{fnfrom(value:Blob)->Self{value.0}}"#,
                r#"implserde::SerializeforBlob{fnserialize<S>(&self,serializer:S)->Result<S::Ok,S::Error>whereS:serde::Serializer,"#,
                r#"{super::support::serialize_base64(&self.0,serializer)}}"#,
                r#"impl<'de>serde::Deserialize<'de>forBlob{fndeserialize<D>(deserializer:D)->Result<Self,D::Error>"#,
                r#"whereD:serde::Deserializer<'de>,{super::support::deserialize_base64(deserializer).map(Blob)}}"#,
            ),
            concat!(
                r#"pubenumNothing{}impl<'de>serde::Deserialize<'de>forNothing{fndeserialize<D>(_deserializer:D)->Result<Self,D::Error>"#,
                r#"whereD:serde::Deserializer<'de>,{Err(serde::de::Error::custom("noJSONvaluecaninhabitschemaNothing"))}}"#,
                r#"implserde::SerializeforNothing{fnserialize<S>(&self,_serializer:S)->Result<S::Ok,S::Error>"#,
                r#"whereS:serde::Serializer,{Err(serde::ser::Error::custom("noJSONvaluecaninhabitschemaNothing"))}}"#,
                r#"#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]#[serde(transparent)]pubstructCycle(pubVec<Cycle>);"#,
            ),
            concat!(
                r#"pubtypePair=(PairItem0,PairItem1);pubtypePairItem0=String;pubtypePairItem1=i64;"#,
                r#"pubtypeAnything=serde_json::Value;pubtypeNil=();"#,
            ),
        ],
    );
}

/// A struct is renamed for XML only where its root element's name differs from its identifier.
#[test]
fn a_struct_is_renamed_for_xml_only_where_its_element_name_differs() {
    let types = types(false, false);
    assert_contains_all(
        &types,
        &[
            r#"#[derive(Debug,Clone,PartialEq,Default,Serialize,Deserialize)]#[serde(rename="root-el")]pubstructRoot{"#,
            r#"#[derive(Debug,Clone,PartialEq,Default,Serialize,Deserialize)]pubstructPlain{"#,
        ],
    );
}

/// Each member's serde attribute and type: a required one as its own type, a required nullable
/// one as an `Option` whose absence is `missing field`, an optional one as an `Option` decoded only when present, an optional
/// nullable one as a `Nullable`; `readOnly` never serialized (nor a member both `readOnly` and
/// `writeOnly`, whatever direction the `testing` module asks), `writeOnly` decoded as absent; a
/// member with a representable `default` decoded as absent like any other, its rustdoc naming the
/// default; and the typed overflow flattened.
#[test]
fn each_member_takes_its_serde_attribute_and_type() {
    let member = |name: &str, attribute: &str, ty: &str| {
        format!("#[serde(rename=\"{name}\"{attribute})]pub{name}:{ty},")
    };
    let optional =
        ",default,skip_serializing_if=\"Option::is_none\",deserialize_with=\"decode_present\",";
    assert_contains_all(
        &types(false, false),
        &[
            &format!(
                "pubstructThing{{///Read-only:theserversetsit,soitisneversentinarequest.{}{}{}",
                member(
                    "id",
                    ",default,skip_serializing,deserialize_with=\"decode_present\",",
                    "Option<ThingId>"
                ),
                member("label", "", "ThingLabel"),
                member(
                    "maybe",
                    ",deserialize_with=\"decode_required\"",
                    "Option<ThingMaybe>"
                ),
            ),
            &format!(
                "///Write-only:theserveracceptsitinrequestsandomitsitfromresponses,soitdecodesasabsent.{}",
                member("secret", optional, "Option<ThingSecret>"),
            ),
            &format!(
                "{}///Read-only:theserversetsit,soitisneversentinarequest.\
                 ///Write-only:theserveracceptsitinrequestsandomitsitfromresponses,soitdecodesasabsent.{}{}\
                 ///Default:`5`.{}///Default:`green`.{}///Default:`3`.{}///Deprecatedperthespec.{}\
                 #[serde(flatten,serialize_with=\"Thing::serialize_overflow\")]pubadditional:BTreeMap<String,ThingAdditional>,}}",
                member(
                    "hint",
                    ",default,skip_serializing_if=\"super::support::Nullable::is_absent\",",
                    "super::support::Nullable<ThingHint>"
                ),
                member(
                    "both",
                    ",default,skip_serializing,deserialize_with=\"decode_present\",",
                    "Option<ThingBoth>"
                ),
                member(
                    "note",
                    ",default,skip_serializing_if=\"super::support::Nullable::is_absent\",",
                    "super::support::Nullable<ThingNote>"
                ),
                member(
                    "count",
                    optional,
                    "Option<ThingCount>"
                ),
                member(
                    "color",
                    optional,
                    "Option<Color>"
                ),
                member(
                    "level",
                    optional,
                    "Option<Level>"
                ),
                member("old", optional, "Option<ThingOld>"),
            ),
        ],
    );
    // A defaulted member decodes as absent, as every other optional member does: no default
    // provider is emitted for serde to call.
    let models = types(false, false);
    assert!(!models.contains("fndefault_"), "{models}");
    // Under `testing`, a member both `readOnly` and `writeOnly` is still never written.
    assert_contains_all(
        &types(false, true),
        &[&member(
            "both",
            ",default,skip_serializing,deserialize_with=\"decode_present\",",
            "Option<ThingBoth>",
        )],
    );
}

/// A required nullable XML attribute leaves its `None` off the wire, as OpenAPI writes a `null`
/// attribute, and decodes an absent one as `None`; a required nullable element keeps its
/// `missing field` decode and writes its `None`.
#[test]
fn a_required_nullable_xml_attribute_skips_its_null() {
    const XML: &str = r#"openapi: 3.1.0
info: {title: T, version: '1'}
paths:
  /receipt:
    post:
      operationId: postReceipt
      requestBody:
        required: true
        content:
          application/xml:
            schema:
              type: object
              xml: {name: Receipt}
              required: [count, note]
              properties:
                count: {type: [number, 'null'], xml: {attribute: true}}
                note: {type: [string, 'null']}
      responses:
        '204': {description: none}
"#;
    for testing in [false, true] {
        let generated = generated_with(XML, |spec| {
            spec.with_testing(crate::TestingModule::from_on(testing))
        });
        assert_contains_all(
            between(&generated, "pubmodtypes{", "pubstructClient{"),
            &[concat!(
                r#"#[serde(rename="@count",skip_serializing_if="Option::is_none")]"#,
                r#"pubcount:Option<PostReceiptBodyCount>,"#,
                r#"#[serde(rename="note",deserialize_with="decode_required")]pubnote:Option<"#,
            )],
        );
    }
}

/// `new` takes every constructed member in declaration order (a required `writeOnly` one stored as
/// present, `null` included) and sets every other member as decoding an absent one does; a
/// setter follows for each member `new` does not take, a nullable one's beside its `_null`
/// setter, and for the overflow map, then a `{field}_or_default` accessor per member with an
/// applied default, returning a `Copy` value as it is and cloning any other. `Default` is derived
/// where `new` takes nothing, and absent otherwise.
#[test]
fn constructors_take_the_constructed_members_and_set_the_rest() {
    let types = types(false, false);
    assert_contains_all(
        &types,
        &[
            concat!(
                r#"implThing{///A`Thing`holdingtherequiredmembersgiven,indeclarationorder,andeveryoptionalmember"#,
                r#"unset,amemberwithaschema`default`included."#,
                r#"#[allow(clippy::too_many_arguments)]#[must_use]pubfnnew(label:ThingLabel,maybe:Option<ThingMaybe>,"#,
                r#"secret:ThingSecret,hint:Option<ThingHint>,)->Self{Self{id:None,label,maybe,secret:Some(secret),"#,
                r#"hint:matchhint{Some(value)=>super::support::Nullable::Value(value),None=>super::support::Nullable::Null,},"#,
                r#"both:None,note:super::support::Nullable::Absent,count:None,"#,
                r#"color:None,level:None,old:None,additional:BTreeMap::new(),}}"#,
                r#"///Setthe`id`member.#[must_use]pubfnwith_id(mutself,value:ThingId)->Self{self.id=Some(value);self}"#,
                r#"///Setthe`both`member.#[must_use]pubfnwith_both(mutself,value:ThingBoth)->Self{self.both=Some(value);self}"#,
                r#"///Setthe`note`member:avalue,or`Nullable::Null`foranexplicit`null`.#[must_use]pubfnwith_note(mutself,"#,
                r#"value:implstd::convert::Into<super::support::Nullable<ThingNote>>,)->Self{self.note=value.into();self}"#,
                r#"///Setthe`note`membertoanexplicit`null`.#[must_use]pubfnwith_note_null(mutself)->Self"#,
                r#"{self.note=super::support::Nullable::Null;self}"#,
            ),
            concat!(
                r#"///Setthemembersnopropertydeclares,replacinganysetbefore.Anentrywhosekeyisadeclaredmember'snameisnotsent."#,
                r#"#[must_use]pubfnwith_additional(mutself,"#,
                r#"value:BTreeMap<String,ThingAdditional>,)->Self{self.additional=value;self}"#,
                r#"///The`count`memberwhereitisset,elseitsschemadefault.//////Default:`5`.#[must_use]"#,
                r#"pubfncount_or_default(&self)->ThingCount{self.count.unwrap_or(5)}"#,
                r#"///The`color`memberwhereitisset,elseitsschemadefault.//////Default:`green`.#[must_use]"#,
                r#"pubfncolor_or_default(&self)->Color{self.color.unwrap_or(Color::Green)}"#,
                r#"///The`level`memberwhereitisset,elseitsschemadefault.//////Default:`3`.#[must_use]"#,
                r#"pubfnlevel_or_default(&self)->Level{self.level.unwrap_or(Level(3))}}"#,
            ),
            "#[derive(Debug,Clone,PartialEq,Default,Serialize,Deserialize)]pubstructLoose{",
            concat!(
                r#"implLoose{///A`Loose`witheveryoptionalmemberunset,amemberwithaschema`default`included."#,
                r#"`Default`buildsthesamevalue.#[allow(clippy::too_many_arguments)]"#,
                r#"#[must_use]pubfnnew()->Self{Self{x:None}}"#,
            ),
            "#[derive(Debug,Clone,PartialEq,Default,Serialize,Deserialize)]pubstructDefaulted{",
            "pubfnnew()->Self{Self{n:None,s:None,b:None}}",
            concat!(
                r#"pubfnn_or_default(&self)->DefaultedN{self.n.unwrap_or(1.5)}"#,
                r#"///The`s`memberwhereitisset,elseitsschemadefault.//////Default:`hi`.#[must_use]"#,
                r#"pubfns_or_default(&self)->DefaultedS{self.s.clone().unwrap_or_else(||"hi".to_owned())}"#,
            ),
            r#"pubfnb_or_default(&self)->DefaultedB{self.b.unwrap_or(true)}"#,
            concat!(
                r#"#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]pubstructOnly{#[serde(rename="a")]puba:OnlyA,}"#,
                r#"implOnly{///A`Only`holdingeverymembergiven,indeclarationorder.#[allow(clippy::too_many_arguments)]"#,
                r#"#[must_use]pubfnnew(a:OnlyA)->Self{Self{a}}}"#,
            ),
        ],
    );
    let thing = between(&types, "pubstructThing{", "pubstructLoose{");
    for absent in [
        "pubfnwith_label(",
        "pubfnwith_secret(",
        "implstd::default::DefaultforThing",
    ] {
        assert!(!thing.contains(absent), "`{absent}` in {thing}");
    }
}
