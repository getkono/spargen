use crate::codegen::test_harness::{assert_contains_all, between, generated};

/// One operation per body codec and shape: a required JSON body (`putJson`), an optional XML one
/// (`putXml`), a text scalar (`putText`), raw octets (`putRaw`), a form with an RFC 6570-mode,
/// a deep-object, a JSON-mode and a text-mode property beside a typed overflow (`putForm`), and
/// a nullable form with one (`putNullableForm`). `Upload` is sent as multipart by `putUpload`,
/// which styles its binary `a`, and `putUploadAgain`, which styles `b`, so each sends one file
/// part and the type's setters merge both.
const BODIES: &str = r##"openapi: 3.1.0
info: {title: T, version: '1'}
components:
  schemas:
    Item: {type: object, properties: {name: {type: string}}}
    Kind: {type: string, enum: [x, y]}
    Upload:
      type: object
      required: [a, b, count]
      properties:
        a: {type: string, format: binary}
        b: {type: string, format: binary}
        count: {type: integer}
        kind: {$ref: '#/components/schemas/Kind'}
        meta: {$ref: '#/components/schemas/Item'}
        note: {type: [string, 'null']}
        stamp: {type: string, readOnly: true}
        tags: {type: array, items: {type: string}}
      additionalProperties: {type: string, format: binary}
    Form:
      type: object
      required: [name]
      properties:
        name: {type: string}
        tags: {type: array, items: {type: string}}
        deep: {$ref: '#/components/schemas/Item'}
        blob: {$ref: '#/components/schemas/Item'}
        plain: {type: integer}
      additionalProperties: {type: integer}
paths:
  /json:
    put:
      operationId: putJson
      requestBody:
        required: true
        content: {application/merge-patch+json: {schema: {$ref: '#/components/schemas/Item'}}}
      responses:
        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/Item'}}}}
  /xml:
    put:
      operationId: putXml
      requestBody:
        content: {application/xml: {schema: {$ref: '#/components/schemas/Item'}}}
      responses:
        '204': {description: done}
  /text:
    put:
      operationId: putText
      requestBody:
        required: true
        content: {text/plain: {schema: {type: integer}}}
      responses:
        '204': {description: done}
  /raw:
    put:
      operationId: putRaw
      requestBody:
        required: true
        content: {application/octet-stream: {schema: {type: string, format: binary}}}
      responses:
        '204': {description: done}
  /form:
    put:
      operationId: putForm
      requestBody:
        required: true
        content:
          application/x-www-form-urlencoded:
            schema: {$ref: '#/components/schemas/Form'}
            encoding:
              tags: {style: spaceDelimited, explode: false, allowReserved: true}
              deep: {style: deepObject, explode: true}
              blob: {contentType: application/json}
              plain: {contentType: text/plain}
      responses:
        '204': {description: done}
  /form/nullable:
    put:
      operationId: putNullableForm
      requestBody:
        required: true
        content:
          application/x-www-form-urlencoded:
            schema:
              type: [object, 'null']
              properties: {a: {type: string}}
              additionalProperties: {type: string}
      responses:
        '204': {description: done}
  /upload:
    put:
      operationId: putUpload
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema: {$ref: '#/components/schemas/Upload'}
            encoding:
              a: {style: form, explode: true}
              count:
                contentType: text/plain
                headers:
                  X-Part-Kind: {schema: {type: string, const: counted}}
              tags:
                style: form
                headers:
                  X-Part-Kind: {schema: {type: string, const: tagged}}
      responses:
        '204': {description: done}
  /upload/again:
    put:
      operationId: putUploadAgain
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema: {$ref: '#/components/schemas/Upload'}
            encoding:
              b: {style: form}
      responses:
        '204': {description: done}
"##;

/// The preparer of the async method `name`, which holds its body send.
fn preparer<'g>(generated: &'g str, name: &str) -> &'g str {
    between(
        generated,
        &format!("asyncfnprepare_{name}("),
        "Ok((request,replay))}",
    )
}

/// Each codec sends its body under its declared `Content-Type`: JSON serialized here, under the
/// media key's own essence; XML through the runtime; a text scalar unquoted; raw octets moved in
/// as they are. A body the document leaves optional is sent only when the caller supplies one.
#[test]
fn each_codec_sends_its_body_under_its_declared_content_type() {
    let generated = generated(BODIES);
    let content_type = "request=request.header(reqwest::header::CONTENT_TYPE,";
    assert_contains_all(
        preparer(&generated, "put_json"),
        &[&format!(
            "letmutrequest=support::RequestBuilder::new(reqwest::Method::PUT,url);\
             request=request.header(reqwest::header::ACCEPT,\"application/json\");\
             letbody=serde_json::to_vec(body).map_err(support::Error::request_construction)?;\
             {content_type}\"application/merge-patch+json\").body(body);letreplay"
        )],
    );
    assert_contains_all(
        preparer(&generated, "put_xml"),
        &[&format!(
            "ifletSome(body)=body{{letbody=support::to_xml(body).map_err(support::Error::widen)?;\
             {content_type}\"application/xml\").body(body);}}letreplay"
        )],
    );
    assert_contains_all(
        preparer(&generated, "put_text"),
        &[&format!(
            "letbody=support::serialize_simple(body,false,support::PercentEncoding::Passthrough,)\
             .map_err(support::Error::request_construction)?;{content_type}\"text/plain\")\
             .body(body);"
        )],
    );
    assert_contains_all(
        preparer(&generated, "put_raw"),
        &[&format!(
            "url);{content_type}\"application/octet-stream\").body(body);letreplay"
        )],
    );
}

/// A form body lists each property's resolved mode: text, JSON, and each RFC 6570 style with its
/// explode and percent-encoding; and it sends its typed overflow in the overflow type's mode,
/// counted through the `Option` of a nullable body.
#[test]
fn a_form_body_sends_each_property_in_its_resolved_mode() {
    let generated = generated(BODIES);
    assert_contains_all(
        preparer(&generated, "put_form"),
        &[
            "constFORM_PROPERTIES:&[support::FormProperty]=&[\
             support::FormProperty{name:\"name\",mode:support::FormMode::Text,},\
             support::FormProperty{name:\"tags\",mode:support::FormMode::Style{\
             style:support::FormStyle::Delimited(support::Delimiter::Space),explode:false,\
             encoding:support::PercentEncoding::Reserved,},},\
             support::FormProperty{name:\"deep\",mode:support::FormMode::Style{\
             style:support::FormStyle::DeepObject,explode:true,\
             encoding:support::PercentEncoding::Form,},},\
             support::FormProperty{name:\"blob\",mode:support::FormMode::Json,},\
             support::FormProperty{name:\"plain\",mode:support::FormMode::Text,},];",
            "support::serialize_form_body(body,FORM_PROPERTIES,\
             Some((support::FormMode::Text,body.additional.keys().filter(|key|{\
             !matches!(key.as_str(),\"name\"|\"tags\"|\"deep\"|\"blob\"|\"plain\")}).count(),)),)",
            "\"application/x-www-form-urlencoded\").body(body);",
        ],
    );
    assert_contains_all(
        preparer(&generated, "put_nullable_form"),
        &[
            "Some((support::FormMode::Text,body.as_ref().map_or(0,|value|{value.additional.keys()\
           .filter(|key|!matches!(key.as_str(),\"a\")).count()},),)),",
        ],
    );
}

/// A multipart body adds one part per sent property in declaration order, `readOnly` ones left
/// out: an RFC 6570-mode property style-serialized into text parts carrying its pinned headers, a
/// binary one a file part named by the caller's `MultipartBody` or its part name, a scalar a text
/// part, a composite a JSON part, each with its resolved `Content-Type` and pinned headers. An
/// optional property adds its part only when set, through `Nullable` where it is also nullable;
/// the typed overflow follows, skipping keys a declared property owns.
#[test]
fn a_multipart_body_adds_one_part_per_sent_property() {
    let generated = generated(BODIES);
    let upload = preparer(&generated, "put_upload");
    let mime =
        |media: &str| format!("part=part.mime_str(\"{media}\").map_err(support::Error::widen)?;");
    let header = |value: &str| {
        format!(
            "letmutpart_headers=reqwest::header::HeaderMap::new();part_headers.insert(\
             reqwest::header::HeaderName::try_from(\"X-Part-Kind\")\
             .map_err(support::Error::request_construction)?,\
             reqwest::header::HeaderValue::try_from(\"{value}\")\
             .map_err(support::Error::request_construction)?,);\
             part=part.headers(part_headers).map_err(support::Error::widen)?;"
        )
    };
    assert_contains_all(
        upload,
        &[
            &format!(
                "letmutform=support::MultipartForm::new();\
                 forvalueinsupport::serialize_multipart_values(&body.a,support::FormStyle::Form,\
                 true,).map_err(support::Error::request_construction)?\
                 {{form=form.part(\"a\",support::MultipartPart::text(value));}}\
                 letmutpart=support::MultipartPart::bytes(body.b.to_vec())\
                 .file_name(body.file_name_of(\"b\").unwrap_or(\"b\"));{}form=form.part(\"b\",part);\
                 letmutpart=support::MultipartPart::text(body.count.to_string());{}{}\
                 form=form.part(\"count\",part);\
                 ifletSome(value)=&body.kind{{letmutpart=support::MultipartPart::text(\
                 value.to_string());{}form=form.part(\"kind\",part);}}\
                 ifletSome(value)=&body.meta{{letmutpart=support::MultipartPart::text(\
                 serde_json::to_string(value).map_err(support::Error::request_construction)?,);\
                 {}form=form.part(\"meta\",part);}}\
                 ifletsupport::Nullable::Value(value)=&body.note{{",
                mime("application/octet-stream"),
                mime("text/plain"),
                header("counted"),
                mime("text/plain"),
                mime("application/json"),
            ),
            &format!(
                "ifletSome(value)=&body.tags{{forvalueinsupport::serialize_multipart_values(\
                 value,support::FormStyle::Form,true,).map_err(support::Error::request_construction)?\
                 {{letmutpart=support::MultipartPart::text(value);{}form=form.part(\"tags\",part);}}}}\
                 for(name,value)in&body.additional{{\
                 ifmatches!(name.as_str(),\"a\"|\"b\"|\"count\"|\"kind\"|\"meta\"|\"note\"|\"stamp\"\
                 |\"tags\"){{continue;}}\
                 letmutpart=support::MultipartPart::bytes(value.to_vec())\
                 .file_name(body.file_name_of(name).unwrap_or(name.as_str()));{}\
                 form=form.part(name.clone(),part);}}request=request.multipart(form);",
                header("tagged"),
                mime("application/octet-stream"),
            ),
        ],
    );
    assert!(!upload.contains("form.part(\"stamp\""), "{upload}");
}

/// A multipart body type's `filename` setters are the union of the file parts every operation
/// sending it declares, in the struct's field order, the overflow's last: `putUpload` sends `b`
/// as a file and `putUploadAgain` sends `a`.
#[test]
fn file_name_setters_merge_every_operations_file_parts_in_field_order() {
    let generated = generated(BODIES);
    let setter = |field: &str| {
        format!(
            "///Send`file_name`asthe`filename`ofthe`{field}`part,inplaceofitspartname.\
             #[must_use]pubfn{field}_file_name(self,file_name:implInto<String>)->Self\
             {{self.file_name_for(\"{field}\",file_name)}}"
        )
    };
    assert_contains_all(
        &generated,
        &[&format!(
            "implsupport::MultipartBody<'_,types::Upload>{{{}{}\
             ///Send`file_name`asthe`filename`ofthe`additionalProperties`part\
             ///keyed`key`,inplaceofitskey.#[must_use]pubfnadditional_file_name(self,\
             key:implInto<String>,file_name:implInto<String>,)->Self\
             {{self.file_name_for(key,file_name)}}}}",
            setter("a"),
            setter("b"),
        )],
    );
}

/// A success body announces its chosen media in `Accept`, a CBOR one and a JSON one alike.
#[test]
fn a_success_body_announces_its_media_in_accept() {
    let generated = generated(
        r##"openapi: 3.1.0
info: {title: T, version: '1'}
paths:
  /cbor:
    get:
      operationId: getCbor
      responses:
        '200': {description: ok, content: {application/cbor: {schema: {type: string, format: binary}}}}
"##,
    );
    assert_contains_all(
        preparer(&generated, "get_cbor"),
        &["request=request.header(reqwest::header::ACCEPT,\"application/cbor\");"],
    );
    let bodies = self::generated(BODIES);
    assert_contains_all(
        preparer(&bodies, "put_json"),
        &["request=request.header(reqwest::header::ACCEPT,\"application/json\");"],
    );
}

/// A reservation never reaches codegen; were one to, the multipart emitter refuses it rather than
/// picking a part shape for a body nobody lowered.
#[test]
#[should_panic(expected = "a reservation reached codegen")]
fn a_reserved_multipart_part_is_refused() {
    let _ = super::media_part_tokens(
        Some(&crate::ir::TypeKind::Reserved),
        &quote::quote! { value },
        &quote::quote! { value },
        &quote::quote! { name },
    );
}
