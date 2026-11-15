use crate::codegen::test_harness::{assert_contains_all, between, generated};

/// `getThing` declares a parameter of every location, style and codec: path parameters in
/// `simple` (reserved and not), `matrix`, `label` and JSON `content`; query parameters in `form`,
/// reserved `form`, both delimited styles, `deepObject`, JSON and text `content`; headers, one
/// JSON; and cookies in `form`, 3.2's `style: cookie` exploded and not, and JSON `content`.
/// `listThings` has only optional parameters, so its method takes its bundle as an `Option`.
/// `lookup`, `lookupMaybe` and `listRecords` carry a whole-query value: required JSON, optional
/// JSON, and form-encoded.
const PARAMS: &str = r##"openapi: 3.2.0
info: {title: T, version: '1'}
components:
  schemas:
    Query: {type: object, properties: {term: {type: string}}}
paths:
  /things/{id}/{raw}/{m}/{l}/{c}:
    get:
      operationId: getThing
      parameters:
        - {name: id, in: path, required: true, schema: {type: string}}
        - {name: raw, in: path, required: true, allowReserved: true, schema: {type: string}}
        - {name: m, in: path, required: true, style: matrix, explode: true, schema: {type: array, items: {type: string}}}
        - {name: l, in: path, required: true, style: label, schema: {type: integer}}
        - {name: c, in: path, required: true, content: {application/json: {schema: {$ref: '#/components/schemas/Query'}}}}
        - {name: q, in: query, required: true, schema: {type: string}}
        - {name: r, in: query, allowReserved: true, explode: false, schema: {type: array, items: {type: string}}}
        - {name: sp, in: query, style: spaceDelimited, schema: {type: array, items: {type: string}}}
        - {name: pp, in: query, style: pipeDelimited, schema: {type: array, items: {type: string}}}
        - {name: deep, in: query, style: deepObject, schema: {$ref: '#/components/schemas/Query'}}
        - {name: cq, in: query, content: {application/json: {schema: {type: [object, 'null'], properties: {a: {type: string}}}}}}
        - {name: tq, in: query, content: {text/plain: {schema: {type: integer}}}}
        - {name: old, in: query, deprecated: true, schema: {type: integer, default: 7}}
        - {name: nq, in: query, schema: {type: [string, 'null']}}
        - {name: X-Req, in: header, required: true, schema: {type: string}}
        - {name: X-Json, in: header, content: {application/json: {schema: {$ref: '#/components/schemas/Query'}}}}
        - {name: ck, in: cookie, schema: {type: string}}
        - {name: ce, in: cookie, style: cookie, explode: true, schema: {type: array, items: {type: string}}}
        - {name: cl, in: cookie, style: cookie, explode: false, schema: {type: array, items: {type: string}}}
        - {name: cj, in: cookie, content: {application/json: {schema: {$ref: '#/components/schemas/Query'}}}}
      responses:
        '204': {description: done}
  /things:
    get:
      operationId: listThings
      parameters:
        - {name: page, in: query, schema: {type: integer}}
        - {name: cn, in: query, content: {application/json: {schema: {type: [object, 'null'], properties: {a: {type: string}}}}}}
      responses:
        '204': {description: done}
  /lookup:
    get:
      operationId: lookup
      parameters:
        - {name: filter, in: querystring, required: true, content: {application/json: {schema: {$ref: '#/components/schemas/Query'}}}}
      responses:
        '204': {description: done}
    post:
      operationId: lookupMaybe
      parameters:
        - {name: filter, in: querystring, content: {application/json: {schema: {$ref: '#/components/schemas/Query'}}}}
      responses:
        '204': {description: done}
  /records:
    get:
      operationId: listRecords
      parameters:
        - {name: filter, in: querystring, content: {application/x-www-form-urlencoded: {schema: {$ref: '#/components/schemas/Query'}}}}
      responses:
        '204': {description: done}
"##;

/// The preparer of the async method `name`, which serializes its parameters.
fn preparer<'g>(generated: &'g str, name: &str) -> &'g str {
    between(
        generated,
        &format!("asyncfnprepare_{name}("),
        "Ok((request,replay))}",
    )
}

/// Each path parameter is rendered in its style and percent-encoded by its own set: `simple`
/// unreserved or, under `allowReserved`, reserved; `matrix` and `label` with their explode; a
/// JSON `content` value as one opaque segment. Each rendering passes `support::path_segment`,
/// which refuses an empty one, before it is substituted.
#[test]
fn each_path_parameter_renders_in_its_style_and_encoding() {
    let generated = generated(PARAMS);
    assert_contains_all(
        preparer(&generated, "get_thing"),
        &[
            r#"path=path.replace("{id}",&support::path_segment(support::serialize_simple(&id,false,support::PercentEncoding::Unreserved,).map_err(support::Error::request_construction)?,).map_err(support::Error::request_construction)?,);"#,
            r#"path=path.replace("{raw}",&support::path_segment(support::serialize_simple(&raw,false,support::PercentEncoding::ReservedPath,).map_err(support::Error::request_construction)?,).map_err(support::Error::request_construction)?,);"#,
            r#"path=path.replace("{m}",&support::path_segment(support::serialize_matrix("m",&m,true,support::PercentEncoding::Unreserved,).map_err(support::Error::request_construction)?,).map_err(support::Error::request_construction)?,);"#,
            r#"path=path.replace("{l}",&support::path_segment(support::serialize_label(&l,false,support::PercentEncoding::Unreserved,).map_err(support::Error::request_construction)?,).map_err(support::Error::request_construction)?,);"#,
            r#"path=path.replace("{c}",&support::path_segment(support::encode(&support::to_json_string(&c).map_err(support::Error::request_construction)?,support::PercentEncoding::Unreserved,),).map_err(support::Error::request_construction)?,);"#,
        ],
    );
}

/// The query string takes the required parameters first, then the optional ones each when set,
/// in declaration order and each in its style: `form` (reserved under `allowReserved`), both
/// delimited styles, `deepObject`, and `content` values encoded once after their codec renders
/// them; an explicit-`null` JSON value is sent whenever it is not absent.
#[test]
fn each_query_parameter_renders_in_its_style_required_ones_first() {
    let generated = generated(PARAMS);
    assert_contains_all(
        preparer(&generated, "get_thing"),
        &[concat!(
            r#"letmutquery:Vec<String>=Vec::new();"#,
            r#"query.extend(support::serialize_form("q",&params.q,true,support::PercentEncoding::Form,).map_err(support::Error::request_construction)?,);"#,
            r#"ifletSome(value)=params.r.as_ref(){query.extend(support::serialize_form("r",value,false,support::PercentEncoding::Reserved,).map_err(support::Error::request_construction)?,);}"#,
            r#"ifletSome(value)=params.sp.as_ref(){query.extend(support::serialize_delimited("sp",value,support::Delimiter::Space,support::PercentEncoding::Form,).map_err(support::Error::request_construction)?,);}"#,
            r#"ifletSome(value)=params.pp.as_ref(){query.extend(support::serialize_delimited("pp",value,support::Delimiter::Pipe,support::PercentEncoding::Form,).map_err(support::Error::request_construction)?,);}"#,
            r#"ifletSome(value)=params.deep.as_ref(){query.extend(support::serialize_deep_object("deep",value,support::PercentEncoding::Form,).map_err(support::Error::request_construction)?,);}"#,
            r#"if!params.cq.is_absent(){letvalue=&params.cq;query.push(format!("{}={}",support::encode("cq",support::PercentEncoding::Form),support::encode(&support::to_json_string(value).map_err(support::Error::request_construction)?,support::PercentEncoding::Form),),);}"#,
            r#"ifletSome(value)=params.tq.as_ref(){query.push(format!("{}={}",support::encode("tq",support::PercentEncoding::Form),support::encode(&support::serialize_simple(value,false,support::PercentEncoding::Passthrough).map_err(support::Error::request_construction)?,support::PercentEncoding::Form),),);}"#,
            r#"ifletSome(value)=params.old.as_ref(){query.extend(support::serialize_form("old",value,true,support::PercentEncoding::Form,).map_err(support::Error::request_construction)?,);}"#,
            r#"ifletSome(value)=params.nq.as_ref(){query.extend(support::serialize_form("nq",value,true,support::PercentEncoding::Form,).map_err(support::Error::request_construction)?,);}"#,
            r#"leturl=support::build_url_on(&self.core,None,&path,&query)"#,
        )],
    );
}

/// Headers are sent verbatim, a JSON one as its serialization; cookies join as one `Cookie`
/// header, each pair held to the rule its style sends it under: a `form` pair percent-encoded, an
/// exploded `style: cookie` pair to the cookie octets, a non-exploded one to their list, and a
/// `content` pair to the cookie octets.
#[test]
fn headers_and_cookies_render_verbatim_under_their_cookie_rules() {
    let generated = generated(PARAMS);
    assert_contains_all(
        preparer(&generated, "get_thing"),
        &[concat!(
            r#"request=request.header("X-Req",support::serialize_simple(&params.x_req,false,support::PercentEncoding::Passthrough,).map_err(support::Error::request_construction)?,);"#,
            r#"ifletSome(value)=params.x_json.as_ref(){request=request.header("X-Json",support::to_json_string(value).map_err(support::Error::request_construction)?,);}"#,
            r#"letmutcookies:Vec<String>=Vec::new();"#,
            r#"ifletSome(value)=params.ck.as_ref(){forpairinsupport::serialize_form("ck",value,true,support::PercentEncoding::Form,).map_err(support::Error::request_construction)?{cookies.push(support::cookie_pair("ck",pair,support::CookieValue::Encoded).map_err(support::Error::request_construction)?,);}}"#,
            r#"ifletSome(value)=params.ce.as_ref(){forpairinsupport::serialize_form("ce",value,true,support::PercentEncoding::Passthrough,).map_err(support::Error::request_construction)?{cookies.push(support::cookie_pair("ce",pair,support::CookieValue::Octets).map_err(support::Error::request_construction)?,);}}"#,
            r#"ifletSome(value)=params.cl.as_ref(){forpairinsupport::serialize_form("cl",value,false,support::PercentEncoding::Passthrough,).map_err(support::Error::request_construction)?{cookies.push(support::cookie_pair("cl",pair,support::CookieValue::OctetList).map_err(support::Error::request_construction)?,);}}"#,
            r#"ifletSome(value)=params.cj.as_ref(){cookies.push(support::cookie_pair("cj",format!("{}={}","cj",support::to_json_string(value).map_err(support::Error::request_construction)?),support::CookieValue::Octets,).map_err(support::Error::request_construction)?,);}"#,
            r#"if!cookies.is_empty(){request=request.header(reqwest::header::COOKIE,cookies.join(";"));}"#,
        )],
    );
}

/// Where every parameter of an operation is optional, its method takes the bundle as an `Option`,
/// so each parameter is read through it. With no cookie parameter, it keeps no cookie jar.
#[test]
fn an_optional_bundle_is_read_through_its_option() {
    let generated = generated(PARAMS);
    let list_things = preparer(&generated, "list_things");
    assert_contains_all(
        list_things,
        &[
            r#"ifletSome(value)=params.as_ref().and_then(|params|params.page.as_ref()){query.extend(support::serialize_form("page",value,true,support::PercentEncoding::Form,)"#,
            r#"ifletSome(value)=params.as_ref().filter(|params|!params.cn.is_absent()).map(|params|&params.cn){query.push(format!("#,
        ],
    );
    assert!(!list_things.contains("cookies"), "{list_things}");
}

/// A whole-query value: a required JSON one is the raw query's initializer, an optional JSON one
/// is set as the one opaque raw query when given, and a form-encoded one is rendered by the form
/// body's serializer through its resolved Encoding Objects into one fragment, leaving the raw
/// query unset; each URL is built with the raw query beside the
/// fragments.
#[test]
fn a_whole_query_value_renders_by_its_codec() {
    let generated = generated(PARAMS);
    let build_url = r#"leturl=support::build_url_with_query_string_on(&self.core,None,&path,&query,raw_query.as_deref(),)"#;
    assert_contains_all(
        preparer(&generated, "lookup"),
        &[&format!(
            "{}{build_url}",
            r#"letmutquery:Vec<String>=Vec::new();letraw_query:Option<String>=Some(support::encode(&support::to_json_string(&params.filter).map_err(support::Error::request_construction)?,support::PercentEncoding::Form,),);"#,
        )],
    );
    assert_contains_all(
        preparer(&generated, "lookup_maybe"),
        &[&format!(
            "{}{build_url}",
            r#"letmutraw_query:Option<String>=None;ifletSome(value)=params.as_ref().and_then(|params|params.filter.as_ref()){raw_query=Some(support::encode(&support::to_json_string(value).map_err(support::Error::request_construction)?,support::PercentEncoding::Form,),);}"#,
        )],
    );
    assert_contains_all(
        preparer(&generated, "list_records"),
        &[&format!(
            "{}{build_url}",
            concat!(
                r#"letraw_query:Option<String>=None;ifletSome(value)=params.as_ref().and_then(|params|params.filter.as_ref()){{letform_value=value;letform_query={"#,
                r#"constQUERYSTRING_PROPERTIES:&[support::FormProperty]=&[support::FormProperty{name:"term",mode:support::FormMode::Text,},];"#,
                r#"support::serialize_form_body(form_value,QUERYSTRING_PROPERTIES,None,).map_err(support::Error::request_construction)?};"#,
                r#"if!form_query.is_empty(){query.push(form_query);}}}"#,
            ),
        )],
    );
}

/// The `…Params` struct holds every parameter but a path one, in declaration order: a required
/// one as its own type, an explicit-`null` one as a `Nullable` of the non-null type, and any other
/// as an `Option`, each with its `deprecated` and `default` notes. Its setters cover the optional
/// ones, taking `impl Into<T>` but where `T` is numeric, an explicit-`null` one beside its `_null`
/// setter, and it converts from its `…Required` struct, which holds the required ones in
/// declaration order; a bundle of optional ones derives `Default` instead.
#[test]
fn the_params_struct_bundles_every_parameter_but_a_path_one() {
    let generated = generated(PARAMS);
    let get_thing = between(
        &generated,
        "pubstructGetThingParams{",
        "///Setthe`X-Json`parameter.",
    );
    assert_contains_all(
        get_thing,
        &[
            concat!(
                r#"pubstructGetThingParams{#[serde(rename="q")]pubq:types::Q,"#,
                r#"#[serde(rename="r",skip_serializing_if="Option::is_none")]pubr:Option<types::R>,"#,
                r#"#[serde(rename="sp",skip_serializing_if="Option::is_none")]pubsp:Option<types::Sp>,"#,
                r#"#[serde(rename="pp",skip_serializing_if="Option::is_none")]pubpp:Option<types::Pp>,"#,
                r#"#[serde(rename="deep",skip_serializing_if="Option::is_none")]pubdeep:Option<types::Query>,"#,
                r#"///`Nullable::Null`sendstheJSON`null`;`Nullable::Absent`omitstheparameter."#,
                r#"#[serde(rename="cq",skip_serializing_if="support::Nullable::is_absent")]pubcq:support::Nullable<types::Cq>,"#,
                r#"#[serde(rename="tq",skip_serializing_if="Option::is_none")]pubtq:Option<types::Tq>,"#,
                r#"///Deprecatedperthespec.///Default:`7`."#,
                r#"#[serde(rename="old",skip_serializing_if="Option::is_none")]pubold:Option<types::Old>,"#,
                r#"#[serde(rename="nq",skip_serializing_if="Option::is_none")]pubnq:Option<types::Nq>,"#,
                r#"#[serde(rename="X-Req")]pubx_req:types::XReq,"#,
            ),
            r#"#[serde(rename="cj",skip_serializing_if="Option::is_none")]pubcj:Option<types::Query>,}"#,
            r#"///Therequiredparametersof`GetThingParams`,whichconvertsfromthiswitheveryoptionaloneunset."#,
            concat!(
                r#"#[derive(Debug,Clone)]pubstructGetThingParamsRequired{///The`q`parameter.pubq:types::Q,"#,
                r#"///The`X-Req`parameter.pubx_req:types::XReq,}"#,
            ),
            concat!(
                r#"implstd::convert::From<GetThingParamsRequired>forGetThingParams{"#,
                r#"///Theparameterswitheveryrequiredonetakenfrom`GetThingParamsRequired`andeveryoptionaloneunset."#,
                r#"fnfrom(required:GetThingParamsRequired)->Self{Self{q:required.q,r:None,sp:None,pp:None,deep:None,"#,
                r#"cq:support::Nullable::Absent,tq:None,old:None,nq:None,x_req:required.x_req,x_json:None,ck:None,"#,
                r#"ce:None,cl:None,cj:None,}}}"#,
            ),
            r#"///Setthe`r`parameter.#[must_use]pubfnr(mutself,value:implInto<types::R>)->Self{self.r=Some(value.into());self}"#,
            concat!(
                r#"///Setthe`cq`parameter.//////Avalue,or`Nullable::Null`tosendtheJSON`null`;leavingtheparameterunsetomitsit."#,
                r#"#[must_use]pubfncq(mutself,value:implInto<support::Nullable<types::Cq>>)->Self{self.cq=value.into();self}"#,
                r#"///Sendthe`cq`parameterastheJSON`null`.#[must_use]pubfncq_null(mutself)->Self{self.cq=support::Nullable::Null;self}"#,
            ),
            r#"///Setthe`nq`parameter.#[must_use]pubfnnq(mutself,value:implInto<types::Nq>)->Self{self.nq=Some(value.into());self}"#,
        ],
    );
    for absent in [
        "pubid:",
        "pubm:",
        "pubfnq(",
        "pubfnx_req(",
        "pubfnid(",
        "pubfnnew(",
    ] {
        assert!(!get_thing.contains(absent), "`{absent}` in {get_thing}");
    }
    assert_contains_all(
        &generated,
        &[
            concat!(
                r#"#[derive(Debug,Clone,Default,serde::Serialize)]pubstructListThingsParams{"#,
                r#"#[serde(rename="page",skip_serializing_if="Option::is_none")]pubpage:Option<types::Page>,"#,
            ),
            concat!(
                r#"pubstructLookupParamsRequired{///The`filter`parameter.pubfilter:types::Query,}"#,
                r#"#[allow(deprecated,missing_docs,unreachable_pub,dead_code,clippy::pedantic,clippy::nursery)]"#,
                r#"implstd::convert::From<LookupParamsRequired>forLookupParams{"#,
                r#"///Theparameterswitheveryrequiredonetakenfrom`LookupParamsRequired`andeveryoptionaloneunset."#,
                r#"fnfrom(required:LookupParamsRequired)->Self{Self{filter:required.filter}}}"#,
            ),
        ],
    );
    // A bundle of only required parameters has no setter, so no inherent `impl`.
    assert!(!generated.contains("implLookupParams{"), "{generated}");
}
