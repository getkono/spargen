use crate::codegen::test_harness::{assert_contains_all, between, generated_with};

/// `getThing` (deprecated) takes a string and an integer path parameter, the integer deprecated
/// with a default, a required nullable string query parameter and a cookie, and sends the bound
/// `X-Tenant` (required) and `X-Trace` (optional) headers. `getFile` returns raw octets, so it has
/// a `_stream` twin; `getParts` does too, beside a bodyless `204`, so its twin returns an enum.
/// `watch` streams. `getPlain` has none of these.
const OPERATIONS: &str = r##"openapi: 3.1.0
info: {title: T, version: '1'}
components:
  schemas:
    Item: {type: object, properties: {name: {type: string}}}
paths:
  /things/{id}/{n}:
    get:
      operationId: getThing
      deprecated: true
      parameters:
        - {name: id, in: path, required: true, schema: {type: string}}
        - {name: n, in: path, required: true, deprecated: true, schema: {type: integer, default: 3}}
        - {name: nr, in: query, required: true, schema: {type: [string, 'null']}}
        - {name: ck, in: cookie, schema: {type: string}}
        - {name: X-Tenant, in: header, required: true, schema: {type: string}}
        - {name: X-Trace, in: header, schema: {type: string}}
      responses:
        '204': {description: done}
  /file:
    get:
      operationId: getFile
      responses:
        '200': {description: ok, content: {application/octet-stream: {schema: {type: string, format: binary}}}}
  /parts:
    get:
      operationId: getParts
      responses:
        '200': {description: ok, content: {application/octet-stream: {schema: {type: string, format: binary}}}}
        '204': {description: empty}
  /watch:
    get:
      operationId: watch
      responses:
        '200': {description: ok, content: {application/x-ndjson: {schema: {$ref: '#/components/schemas/Item'}}}}
  /plain:
    get:
      operationId: getPlain
      responses:
        '200': {description: ok, content: {application/json: {schema: {$ref: '#/components/schemas/Item'}}}}
"##;

fn generated() -> String {
    generated_with(OPERATIONS, |spec| {
        spec.bind_header("X-Tenant").bind_header("X-Trace")
    })
}

/// The async method, its preparer and companions: the arguments in URL template order with a
/// plain string path parameter taken as `impl Into`, the bound headers read from the client, a
/// cookie jar only where a cookie is declared, the request builder and decoder with their docs,
/// and the URL accessor building the same URL.
#[test]
fn a_method_its_preparer_and_companions_share_one_signature() {
    let generated = generated();
    assert_contains_all(
        &generated,
        &[
            concat!(
                r#"asyncfnprepare_get_thing(&self,id:implInto<types::Id>,n:types::N,params:GetThingParams,)"#,
                r#"->Result<(http::Request<support::HttpBody>,Option<support::Replay>),support::Error<std::convert::Infallible>,>"#,
                r#"{letid:types::Id=id.into();letmutpath="/things/{id}/{n}".to_owned();"#,
            ),
            concat!(
                r#"letmutrequest=support::RequestBuilder::new(reqwest::Method::GET,url);"#,
                r#"matchself.core.bound_header("X-Tenant"){Some(value)=>{request=request.sensitive_header("X-Tenant",value);}"#,
                r#"None=>{returnErr(support::Error::RequestConstruction(support::RequestError::MissingBoundParameter{name:"X-Tenant",}),);}}"#,
                r#"ifletSome(value)=self.core.bound_header("X-Trace"){request=request.sensitive_header("X-Trace",value);}"#,
                r#"letmutcookies:Vec<String>=Vec::new();"#,
            ),
            concat!(
                r#"letmutrequest=support::prepare(&self.core,request,false);request.extensions_mut().insert(ops::GET_THING);Ok((request,replay))}"#,
                r#"///Response`204`:done///Parameter`n`isdeprecatedperthespec.///Parameter`n`default:`3`."#,
                r#"///Header`X-Tenant`issentfromthevalueboundwith`with_bound_header`(required,failingthecallwhilenoneisbound)."#,
                r#"///Header`X-Trace`issentfromthevalueboundwith`with_bound_header`(optional,leftoffwhilenoneisbound)."#,
                r#"#[deprecated]#[inline]pubasyncfnget_thing(&self,id:implInto<types::Id>,n:types::N,params:GetThingParams,)"#,
                r#"->Result<support::ResponseValue<()>,support::Error<GetThingError>>"#,
                r#"{let(request,replay)=self.prepare_get_thing(id,n,params).await.map_err(support::Error::widen)?;"#,
                r#"letresponse=support::execute(&self.core,request,replay,false).await.map_err(support::Error::widen)?;"#,
                r#"self.decode_get_thing(response).await}"#,
                r#"///Therequest`get_thing`sends,builtexactlyasitbuildsit—itsURL,headers,credentialsandbody,"#,
                r#"theclient's`RequestOptions`headersandextensions,andits`ops::Operation`amongtheextensions—"#,
                r#"withoutsendingit,foracallerthatsendsititself(signingit,pacingit,oroveranothertransport)"#,
                r#"andhandstheresponsetoitsdecoder.Nothingisreplayedafterarejectedrefreshablecredential:"#,
                r#"thatis`get_thing`'sownsend.#[deprecated]#[inline]pubasyncfnget_thing_request(&self,"#,
                r#"id:implInto<types::Id>,n:types::N,params:GetThingParams,)"#,
                r#"->Result<http::Request<support::HttpBody>,support::Error<std::convert::Infallible>,>"#,
                r#"{self.prepare_get_thing(id,n,params).await.map(|(request,_)|request)}"#,
                r#"///Decodearesponsetotherequest`get_thing`sends(`ops::GET_THING`),obtainedhoweverthecallersentit,"#,
                r#"exactlyas`get_thing`decodesitsown:thesamestatusdispatch,typederror,andbodylimitsfromthis"#,
                r#"client'sconfiguration.`get_thing`itselfsendsitsrequestbuilder'srequestthroughtheclient's"#,
                r#"backend,replaysitoncewherearefreshablecredentialwasrejected,anddecodestheresponsewiththis."#,
                r#"#[deprecated]#[inline]pubasyncfndecode_get_thing(&self,response:http::Response<support::HttpBody>,)"#,
                r#"->Result<support::ResponseValue<()>,support::Error<GetThingError>>"#,
                r#"{letresponse=support::with_body_limits(&self.core,response);"#,
            ),
            concat!(
                r#"#[deprecated]#[inline]pubfnget_thing_url(&self,id:implInto<types::Id>,n:types::N,params:GetThingParams,)"#,
                r#"->Result<url::Url,support::Error<std::convert::Infallible>>{letid:types::Id=id.into();"#,
                r#"letmutpath="/things/{id}/{n}".to_owned();"#,
            ),
            // A required nullable string stays concrete, so `None` needs no annotation.
            r#"pubfnnew(nr:Option<types::Nr>)->Self{Self{nr,ck:None}}"#,
        ],
    );
    let plain = between(
        &generated,
        "asyncfnprepare_get_plain(",
        "Ok((request,replay))}",
    );
    assert!(!plain.contains("cookies"), "{plain}");
}

/// The blocking client forwards each method, request builder and URL accessor to the async one
/// under the same signature, attributes and deprecation.
#[test]
fn the_blocking_client_forwards_under_the_same_signature() {
    let generated = generated();
    let blocking = &generated[generated
        .find("implBlockingClient{")
        .expect("the blocking client")..];
    assert_contains_all(
        blocking,
        &[
            concat!(
                r#"///Header`X-Trace`issentfromthevalueboundwith`with_bound_header`(optional,leftoffwhilenoneisbound)."#,
                r#"#[deprecated]#[inline]pubfnget_thing(&self,id:implInto<types::Id>,n:types::N,params:GetThingParams,)"#,
                r#"->Result<support::ResponseValue<()>,support::Error<GetThingError>>"#,
                r#"{self.runtime.block_on(self.inner.get_thing(id,n,params))}"#,
                r#"///Therequest`get_thing`sends,builtwithoutsendingit(mirrorstheasyncclient's`get_thing_request`)."#,
                r#"#[deprecated]#[inline]"#,
            ),
            concat!(
                r#"pubfnget_thing_url(&self,id:implInto<types::Id>,n:types::N,params:GetThingParams,)"#,
                r#"->Result<url::Url,support::Error<std::convert::Infallible>>{self.inner.get_thing_url(id,n,params)}"#,
            ),
        ],
    );
}

/// A raw binary success has a `_stream` twin sending the method's request exempt from the total
/// timeout and handing the body over unread; beside a bodyless status, the twin returns its own
/// enum. A streaming success is read with no body limits, which every other decoder applies.
#[test]
fn a_binary_success_has_a_stream_twin_and_a_stream_reads_without_body_limits() {
    let generated = generated();
    assert_contains_all(
        &generated,
        &[
            concat!(
                r#"asyncfnprepare_get_file(&self,streaming:bool,)"#,
                r#"->Result<(http::Request<support::HttpBody>,Option<support::Replay>),support::Error<std::convert::Infallible>,>"#,
            ),
            r#"letmutrequest=support::prepare(&self.core,request,streaming);request.extensions_mut().insert(ops::GET_FILE);"#,
            r#"{let(request,replay)=self.prepare_get_file(false).await"#,
            concat!(
                r#"///`get_file`,withitssuccessbodyhandedoverunreadasa`ByteStream`,toreadasitarrives,"#,
                r#"orwholewith`collect_bounded`underalimitofthecaller'schoosing.Therequestandtheerror"#,
                r#"branchare`get_file`'s.Likean`EventStream`,thecallisexemptfromtheclient'stotaltimeout"#,
                r#"anditsbodylimits;itsconnectandreadtimeoutsstillapply.Itsendsitsrequestbuilder's"#,
                r#"requestthroughtheclient'sbackend,replaysitoncewherearefreshablecredentialwasrejected,"#,
                r#"anddecodestheresponsewithitsdecoder.#[inline]pubasyncfnget_file_stream(&self,)"#,
                r#"->Result<support::ResponseValue<support::ByteStream>,support::Error<GetFileError>,>"#,
                r#"{let(request,replay)=self.prepare_get_file(true).await.map_err(support::Error::widen)?;"#,
                r#"letresponse=support::execute(&self.core,request,replay,true).await.map_err(support::Error::widen)?;"#,
                r#"self.decode_get_file_stream(response).await}"#,
                r#"///Therequest`get_file_stream`sends,builtwithoutsendingit:`get_file`'srequest,exemptfrom"#,
                r#"theclient'stotaltimeoutas`get_file_stream`'sis,foracallerthatsendsititselfandhands"#,
                r#"theresponseto`get_file_stream`'sdecoder.Nothingisreplayedafterarejectedrefreshable"#,
                r#"credential:thatis`get_file_stream`'sownsend.#[inline]pubasyncfnget_file_stream_request(&self,)"#,
                r#"->Result<http::Request<support::HttpBody>,support::Error<std::convert::Infallible>,>"#,
                r#"{self.prepare_get_file(true).await.map(|(request,_)|request)}"#,
                r#"///Decodearesponsetotherequest`get_file_stream`sends(`ops::GET_FILE`),obtainedhoweverthe"#,
                r#"callersentit,exactlyas`get_file_stream`decodesitsown:asuccessbodyhandedoverunreadasa"#,
                r#"`ByteStream`,exemptfromtheclient'sbodylimits,andanerrorstatusdispatchedintothe"#,
                r#"operation'stypederror.#[inline]pubasyncfndecode_get_file_stream(&self,"#,
                r#"response:http::Response<support::HttpBody>,)"#,
                r#"->Result<support::ResponseValue<support::ByteStream>,support::Error<GetFileError>,>"#,
                r#"{ifresponse.status().is_success(){Ok(support::stream_success_bytes(response.map(support::read_as_it_arrives)))}"#,
            ),
            concat!(
                r#"///`get_parts`,witheachrawbinarysuccessbodyhandedoverunreadasa`ByteStream`initsstatus'svariant,"#,
                r#"toreadasitarrives,orwholewith`collect_bounded`underalimitofthecaller'schoosing.Therequest"#,
                r#"andtheerrorbranchare`get_parts`'s.Likean`EventStream`,thecallisexemptfromtheclient's"#,
                r#"totaltimeoutanditsbodylimits;itsconnectandreadtimeoutsstillapply.Everyothersuccessbody"#,
                r#"isreadwholeundertheclient'sbodylimits,as`get_parts`readsit.Itsendsitsrequestbuilder's"#,
                r#"requestthroughtheclient'sbackend,replaysitoncewherearefreshablecredentialwasrejected,"#,
                r#"anddecodestheresponsewithitsdecoder.#[inline]pubasyncfnget_parts_stream(&self,)"#,
                r#"->Result<support::ResponseValue<GetPartsStreamResponse>,support::Error<GetPartsError>,>"#,
            ),
            concat!(
                r#"///Decodearesponsetotherequest`get_parts_stream`sends(`ops::GET_PARTS`),obtainedhoweverthe"#,
                r#"callersentit,exactlyas`get_parts_stream`decodesitsown:arawbinarysuccessbodyhandedover"#,
                r#"unreadasa`ByteStream`,exemptfromtheclient'sbodylimits,anyothersuccessbodyreadwhole"#,
                r#"withinthem,andanerrorstatusdispatchedintotheoperation'stypederror."#,
            ),
            concat!(
                r#"pubasyncfndecode_watch(&self,response:http::Response<support::HttpBody>,)"#,
                r#"->Result<support::EventStream<types::Item>,support::Error<WatchError>>"#,
                r#"{ifresponse.status().is_success(){"#,
            ),
        ],
    );
}

/// The `ops` module names each operation's identity, in declaration order.
#[test]
fn the_ops_module_names_each_operations_identity() {
    assert_contains_all(
        &generated(),
        &[concat!(
            r#"pubpath_template:&'staticstr,}"#,
            r#"///`get_thing`:`GET/things/{id}/{n}`.pubconstGET_THING:Operation=Operation{id:"getThing",method:"GET",path_template:"/things/{id}/{n}",};"#,
            r#"///`get_file`:`GET/file`.pubconstGET_FILE:Operation=Operation{id:"getFile",method:"GET",path_template:"/file",};"#,
        )],
    );
}
