use crate::codegen::test_harness::{assert_contains_all, between, generated_with};

/// One union per strategy and shape. `Event` is discriminated over two tagged members, `Stop`
/// under two tags. `Notice` adds an untagged object member and a string member its category
/// selects. `Outcome` falls back to `Fallback` under 3.2's `defaultMapping`, beside an untagged
/// member. `Lone` has an untagged member and a fallback whose tags no other member claims.
/// `Choice` is an `anyOf` tried member by member, one member nullable and one the union `Event`;
/// `Exact` is a `oneOf` tried the same way; `Scalar` is dispatched by JSON type.
const UNIONS: &str = r##"openapi: 3.2.0
info: {title: T, version: '1'}
components:
  schemas:
    Tick: {type: object, required: [kind, n], properties: {kind: {type: string}, n: {type: integer}}}
    Stop: {type: object, required: [kind], properties: {kind: {type: string}}}
    Fallback: {type: object, required: [detail], properties: {detail: {type: string}}}
    Event:
      oneOf: [{$ref: '#/components/schemas/Tick'}, {$ref: '#/components/schemas/Stop'}]
      discriminator:
        propertyName: kind
        mapping: {tick: '#/components/schemas/Tick', stop: '#/components/schemas/Stop', halt: '#/components/schemas/Stop'}
    Notice:
      oneOf:
        - $ref: '#/components/schemas/Tick'
        - {type: object, required: [kind, note], properties: {kind: {type: string}, note: {type: string}}}
        - {type: string}
      discriminator: {propertyName: kind, mapping: {tick: '#/components/schemas/Tick'}}
    Outcome:
      oneOf:
        - $ref: '#/components/schemas/Tick'
        - $ref: '#/components/schemas/Fallback'
        - {type: object, required: [kind, body], properties: {kind: {type: string}, body: {type: string}}}
      discriminator:
        propertyName: kind
        mapping: {tick: '#/components/schemas/Tick'}
        defaultMapping: '#/components/schemas/Fallback'
    Lone:
      oneOf:
        - $ref: '#/components/schemas/Fallback'
        - {type: object, required: [kind, body], properties: {kind: {type: string}, body: {type: string}}}
      discriminator:
        propertyName: kind
        defaultMapping: '#/components/schemas/Fallback'
    Choice:
      anyOf:
        - $ref: '#/components/schemas/Tick'
        - {type: [object, 'null'], required: [note], properties: {note: {type: string}}}
        - $ref: '#/components/schemas/Event'
    Exact:
      oneOf: [{$ref: '#/components/schemas/Tick'}, {$ref: '#/components/schemas/Stop'}]
    Scalar:
      oneOf: [{type: string}, {type: integer}]
paths:
  /unions:
    post:
      operationId: postUnions
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                event: {$ref: '#/components/schemas/Event'}
                notice: {$ref: '#/components/schemas/Notice'}
                outcome: {$ref: '#/components/schemas/Outcome'}
                lone: {$ref: '#/components/schemas/Lone'}
                choice: {$ref: '#/components/schemas/Choice'}
                exact: {$ref: '#/components/schemas/Exact'}
                scalar: {$ref: '#/components/schemas/Scalar'}
      responses:
        '204': {description: done}
"##;

/// The `types` module `UNIONS` generates, with `open_unions` as `open` says.
fn types(open: bool) -> String {
    let generated = generated_with(UNIONS, |spec| {
        spec.with_open_unions(crate::UnionTags::from_on(open))
    });
    between(&generated, "pubmodtypes{", "pubstructClient{").to_owned()
}

/// The declaration and serde impls of union `name` in `types`, up to the next derived item: one
/// deriving `Debug`, as every generated type does, so the local `Tagged` helper a discriminated
/// union's `Serialize` derives does not end it.
fn union<'t>(types: &'t str, name: &str) -> &'t str {
    between(types, &format!("pubenum{name}{{"), "#[derive(Debug")
}

/// A discriminated union decodes by tag, each arm matching every tag that selects its member, and
/// fails on a tag it does not list; on serialize it writes a member's first tag where the member
/// wrote none and refuses one that would not select it again. With no untagged member it checks
/// nothing else and keeps no fallback.
#[test]
fn a_discriminated_union_dispatches_by_tag_and_refuses_a_foreign_one() {
    let types = types(false);
    let event = union(&types, "Event");
    assert_contains_all(
        event,
        &[
            "pubenumEvent{Tick(Box<Tick>),Stop(Box<Stop>),}",
            concat!(
                r#"matchtag.as_str(){"tick"|"Tick"=>{serde_json::from_value(value).map(Event::Tick).map_err(serde::de::Error::custom)}"#,
                r#""stop"|"halt"|"Stop"=>{serde_json::from_value(value).map(Event::Stop).map_err(serde::de::Error::custom)}"#,
                r#"_=>{Err(serde::de::Error::custom("unknowndiscriminatorvalueforunionEvent",),)}}"#,
            ),
            concat!(
                r#"letnon_finite=matchself{Event::Tick(inner)=>super::support::holds_non_finite(inner),"#,
                r#"Event::Stop(inner)=>super::support::holds_non_finite(inner),};"#,
                r#"let(mutvalue,tag):(serde_json::Value,Option<&str>)=matchself{"#,
                r#"Event::Tick(inner)=>{(serde_json::to_value(inner).map_err(serde::ser::Error::custom)?,Some("tick"),)}"#,
                r#"Event::Stop(inner)=>{(serde_json::to_value(inner).map_err(serde::ser::Error::custom)?,Some("stop"),)}};"#,
                r#"letwrote_tag=value.get("kind").is_some();"#,
            ),
            concat!(
                r#"ifletSome(held)=map.get("kind"){letrefusal:Option<&str>=matchself{"#,
                r#"Event::Tick(_)=>{matchheld.as_str(){Some("tick")|Some("Tick")=>None,_=>Some("doesnotselectvariantEvent::Tick"),}}"#,
                r#"Event::Stop(_)=>{matchheld.as_str(){Some("stop")|Some("halt")|Some("Stop")=>None,_=>Some("doesnotselectvariantEvent::Stop"),}}};"#,
            ),
            concat!(
                r#"}else{map.insert("kind".to_owned(),serde_json::Value::String(tag.to_owned()),);}}"#,
                r#"ifnon_finite{#[derive(serde::Serialize)]structTagged<'a,T>{#[serde(rename="kind")]tag:&'astr,#[serde(flatten)]inner:&'aT,}"#,
                r#"returnmatchself{Self::Tick(inner)=>{matchtag.filter(|_|!wrote_tag){Some(tag)=>Tagged{tag,inner}.serialize(serializer),None=>inner.serialize(serializer),}}"#,
                r#"Self::Stop(inner)=>{matchtag.filter(|_|!wrote_tag){Some(tag)=>Tagged{tag,inner}.serialize(serializer),None=>inner.serialize(serializer),}}};}"#,
                r#"value.serialize(serializer)}}"#,
            ),
        ],
    );
    for absent in [
        "iflet(Some(refusal),Some(held))",
        "unknown_fallback",
        "Unknown",
    ] {
        assert!(!event.contains(absent), "`{absent}` in {event}");
    }
}

/// A non-object member is selected by its JSON category before any tag is read; an untagged
/// object member is tried, under its category guard, when the tag is absent or unlisted; and on
/// serialize an untagged member holding a tag another member claims is refused.
#[test]
fn untagged_members_are_selected_by_category_or_by_trial() {
    let types = types(false);
    let notice = union(&types, "Notice");
    let trial = concat!(
        r#"{letmutmatch_count=0_usize;letmutselected:Option<(u32,Self)>=None;"#,
        r#"ifletSome(Ok(inner))=value.is_object().then(||serde_json::from_value::<Box<NoticeVariant1>,>(value.clone()))"#,
        r#"{match_count+=1;letreplace=match&selected{Some((selected_priority,_))=>1042u32>*selected_priority,None=>true,};"#,
        r#"ifreplace{selected=Some((1042u32,Notice::NoticeVariant1(inner)));}}"#,
        r#"matchselected{Some((_,selected))ifmatch_count==1=>Ok(selected),_=>"#,
    );
    assert_contains_all(
        notice,
        &[
            r#"ifvalue.is_string(){returnserde_json::from_value(value).map(Notice::NoticeVariant2).map_err(serde::de::Error::custom);}"#,
            &format!(
                "letSome(tag)=tagelse{{return{trial}{}",
                r#"{Err(serde::de::Error::custom("missingdiscriminatorfield`kind`forunionNotice",),)}}};};"#,
            ),
            &format!(
                "_=>{trial}{}",
                r#"{Err(serde::de::Error::custom("unknowndiscriminatorvalueforunionNotice",),)}}}"#,
            ),
            concat!(
                r#"Notice::Tick(_)=>{matchheld.as_str(){Some("tick")|Some("Tick")=>None,_=>Some("doesnotselectvariantNotice::Tick"),}}"#,
                r#"Notice::NoticeVariant1(_)=>None,Notice::NoticeVariant2(_)=>None,};"#,
            ),
            concat!(
                r#"letrefusal:Option<&str>=matchself{Notice::NoticeVariant1(_)=>{Some("doesnotselectvariantNotice::NoticeVariant1")}_=>None,};"#,
                r#"iflet(Some(refusal),Some(held))=(refusal,value.get("kind")){ifmatches!(held.as_str(),Some("tick")|Some("Tick")){"#,
            ),
        ],
    );
    assert!(!notice.contains("unknown_fallback"), "{notice}");
}

/// Under 3.2's `defaultMapping`, an absent or unlisted tag falls back to its member once the
/// untagged members' trial takes nothing; on serialize the fallback member refuses a tag another
/// member claims, and a payload the trial would take, naming the member it decodes as. A
/// fallback whose tag no other member shares refuses nothing.
#[test]
fn a_default_mapping_falls_back_after_the_trial_and_guards_its_payload() {
    let types = types(false);
    assert_contains_all(
        union(&types, "Outcome"),
        &[
            concat!(
                r#"matchselected{Some((_,selected))ifmatch_count==1=>Ok(selected),"#,
                r#"_=>{serde_json::from_value(value).map(Outcome::Fallback).map_err(serde::de::Error::custom)}}"#,
            ),
            r#""Fallback"=>{serde_json::from_value(value).map(Outcome::Fallback).map_err(serde::de::Error::custom)}"#,
            concat!(
                r#"Outcome::Fallback(_)=>{matchheld.as_str(){Some("tick")|Some("Tick")=>{Some("doesnotselectvariantOutcome::Fallback")}_=>None,}}"#,
                r#"Outcome::OutcomeVariant2(_)=>None,};"#,
            ),
            concat!(
                r#"ifmatches!(self,Outcome::Fallback(_))&&!matches!(value.get("kind").and_then(serde_json::Value::as_str),"#,
                r#"Some("tick")|Some("Tick")|Some("Fallback")){letmutmatch_count=0_usize;"#,
            ),
            concat!(
                r#"letclaimant=matchselected{Some((_,selected))ifmatch_count==1=>Some(selected),_=>None,};"#,
                r#"ifletSome(claimant)=claimant{letclaimant=matchclaimant{Outcome::Tick(_)=>"Outcome::Tick","#,
                r#"Outcome::Fallback(_)=>"Outcome::Fallback",Outcome::OutcomeVariant2(_)=>"Outcome::OutcomeVariant2",};"#,
                r#"returnErr(serde::ser::Error::custom(format_args!("{}{}","payloadofvariantOutcome::Fallbackdecodesasvariant",claimant),),);}}"#,
            ),
        ],
    );
    assert_contains_all(union(&types, "Lone"), &["Lone::Fallback(_)=>None,"]);
}

/// A union with no discriminator tries every member, each under its category guard and, under
/// `anyOf`, an object struct only on a payload holding no member it does not declare, the higher
/// priority winning, and holds the count to its applicator, `anyOf` at least one and `oneOf`
/// exactly one, on decode and on serialize alike; one of statically disjoint JSON types is
/// dispatched by type.
#[test]
fn an_untagged_union_tries_each_member_under_its_applicator() {
    let types = types(false);
    let choice = union(&types, "Choice");
    assert_contains_all(
        choice,
        &[
            concat!(
                r#"letmutmatch_count=0_usize;letmutselected:Option<(u32,Self)>=None;"#,
                r#"ifletSome(Ok(inner))=(value.is_object()&&value.as_object().is_none_or(|members|{members.keys().all(|key|matches!(key.as_str(),"kind"|"n"))})).then(||serde_json::from_value::<Box<Tick>>(value.clone()))"#,
                r#"{match_count+=1;letreplace=match&selected{Some((selected_priority,_))=>1042u32>*selected_priority,None=>true,};"#,
                r#"ifreplace{selected=Some((1042u32,Choice::Tick(inner)));}}"#,
                r#"ifletSome(Ok(inner))=(value.is_object()&&value.as_object().is_none_or(|members|{members.keys().all(|key|matches!(key.as_str(),"note"))})).then(||serde_json::from_value::<Box<ChoiceVariant1>>(value.clone()))"#,
                r#"{match_count+=1;letreplace=match&selected{Some((selected_priority,_))=>1021u32>*selected_priority,None=>true,};"#,
                r#"ifreplace{selected=Some((1021u32,Choice::ChoiceVariant1(inner)));}}"#,
                r#"ifletOk(inner)=serde_json::from_value::<Box<Event>>(value.clone()){match_count+=1;"#,
            ),
            concat!(
                r#"ifmatch_count>=1{selected.map(|(_,value)|value).ok_or_else(||serde::de::Error::custom("#,
                r#""datamustmatchatleastonetypedvariantofunionChoicethatdeclareseverymemberitholds",))}else{Err("#,
            ),
            concat!(
                r#"letmutmatch_count=0_usize;if(value.is_object()&&value.as_object().is_none_or(|members|{members.keys().all(|key|matches!(key.as_str(),"kind"|"n"))}))&&serde_json::from_value::<Box<Tick>>(value.clone()).is_ok(){match_count+=1;}"#,
                r#"if(value.is_object()&&value.as_object().is_none_or(|members|{members.keys().all(|key|matches!(key.as_str(),"note"))}))&&serde_json::from_value::<Box<ChoiceVariant1>>(value.clone()).is_ok(){match_count+=1;}"#,
                r#"ifserde_json::from_value::<Box<Event>>(value.clone()).is_ok(){match_count+=1;}"#,
                r#"ifmatch_count>=1{value.serialize(serializer)}"#,
            ),
        ],
    );
    assert!(!choice.contains("unknown"), "{choice}");
    assert_contains_all(
        union(&types, "Exact"),
        &[
            r#"ifmatch_count==1{selected.map(|(_,value)|value)"#,
            r#"ifmatch_count==1{value.serialize(serializer)}else{Err(serde::ser::Error::custom("serializedvaluemustmatchexactlyonetypedvariantofunionExact",),)}"#,
        ],
    );
    assert_contains_all(
        union(&types, "Scalar"),
        &[concat!(
            r#"ifvalue.is_string(){returnserde_json::from_value(value).map(Scalar::ScalarVariant0).map_err(serde::de::Error::custom);}"#,
            r#"ifvalue.is_number(){returnserde_json::from_value(value).map(Scalar::ScalarVariant1).map_err(serde::de::Error::custom);}"#,
            r#"Err(serde::de::Error::custom("datadidnotmatchanyvariantofunionScalar",),)}}"#,
            r#"implserde::SerializeforScalar{fnserialize<S>(&self,serializer:S)->Result<S::Ok,S::Error>whereS:serde::Serializer,"#,
            r#"{matchself{Scalar::ScalarVariant0(inner)=>inner.serialize(serializer),Scalar::ScalarVariant1(inner)=>inner.serialize(serializer),}}}"#,
        )],
    );
}

/// Under `open_unions`, an unlisted tag decodes into `Unknown`, written back as it arrived:
/// directly where no member is untagged, and only once the untagged members' trial takes nothing
/// otherwise. A union holding an open member counts that member's `Unknown` only where no member
/// matches outright, on decode and on serialize alike.
#[test]
fn an_open_union_keeps_an_unlisted_tag_in_unknown() {
    let types = types(true);
    assert_contains_all(
        union(&types, "Event"),
        &[
            concat!(
                r#"Stop(Box<Stop>),///Apayloadwhose`kind`namesakindthisdescriptiondoesnotlist."#,
                r#"Decodingproducesitonlyforavaluenoothervarianttakes.Unknown{///The`kind`valuethepayloadcarries."#,
                r#"tag:String,///Thewholepayload,asitarrived.value:serde_json::Value,},}"#,
            ),
            r#"_=>Ok(Event::Unknown{tag,value}),}"#,
            r#"Event::Unknown{tag,value}=>(value.clone(),Some(tag.as_str())),};"#,
            r#"Event::Unknown{..}=>None,};"#,
        ],
    );
    assert_contains_all(
        union(&types, "Notice"),
        &[concat!(
            r#"_=>{ifmatch_count==0{Ok(Notice::Unknown{tag,value})}else{Err(serde::de::Error::custom("#,
            r#""datamatchedmorethanoneuntaggedvariantofunionNotice",),)}}"#,
        )],
    );
    assert_contains_all(
        union(&types, "Choice"),
        &[
            r#"letmutselected:Option<(u32,Self)>=None;letmutunknown_fallback:Option<Self>=None;"#,
            concat!(
                r#"ifletOk(inner)=serde_json::from_value::<Box<Event>>(value.clone()){ifmatches!(*inner,Event::Unknown{..})"#,
                r#"{ifunknown_fallback.is_none(){unknown_fallback=Some(Choice::Event(inner));}}else{match_count+=1;"#,
            ),
            concat!(
                r#"}elseifmatch_count==0{unknown_fallback.ok_or_else(||serde::de::Error::custom("#,
                r#""datamustmatchatleastonetypedvariantofunionChoicethatdeclareseverymemberitholds",))}"#,
            ),
            concat!(
                r#"letmutmatch_count=0_usize;letmutunknown_match=false;"#,
                r#"if(value.is_object()&&value.as_object().is_none_or(|members|{members.keys().all(|key|matches!(key.as_str(),"kind"|"n"))}))&&serde_json::from_value::<Box<Tick>>(value.clone()).is_ok(){match_count+=1;}"#,
                r#"if(value.is_object()&&value.as_object().is_none_or(|members|{members.keys().all(|key|matches!(key.as_str(),"note"))}))&&serde_json::from_value::<Box<ChoiceVariant1>>(value.clone()).is_ok(){match_count+=1;}"#,
                r#"ifletOk(inner)=serde_json::from_value::<Box<Event>>(value.clone()){ifmatches!(*inner,Event::Unknown{..})"#,
                r#"{unknown_match=true;}else{match_count+=1;}}ifmatch_count>=1||(match_count==0&&unknown_match)"#,
            ),
        ],
    );
}
