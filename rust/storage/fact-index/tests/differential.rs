//! Differential decisions against the pinned SG-10 in-memory pilot. This is
//! source coverage for the integration owner; this recipient does not run it.
use slophawk_clone_index as legacy;
use slophawk_fact_index as durable;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64,Ordering};
static SERIAL:AtomicU64=AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {fn new()->Self {let p=std::env::temp_dir().join(format!("sgmig09-differential-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));fs::create_dir_all(&p).unwrap();Self(p)} fn db(&self)->PathBuf {self.0.join("index.db")}}
impl Drop for Scratch {fn drop(&mut self) {let _=fs::remove_dir_all(&self.0);}}
fn t(kind:durable::TokenKind,text:&str)->durable::Token {durable::Token {kind,text:text.into()}}
fn seq(local:&str)->Vec<durable::Token> {
    vec![t(durable::TokenKind::Other,"if"),t(durable::TokenKind::LocalIdentifier,local),
         t(durable::TokenKind::Other,"("),t(durable::TokenKind::BoundIdentifier,"Console"),
         t(durable::TokenKind::Literal,"\"literal\""),t(durable::TokenKind::LocalIdentifier,local)]
}
fn doc(id:&str,version:&str,tokens:Vec<durable::Token>)->durable::Document {
    durable::Document {document_id:id.into(),context_key:"target".into(),source_id:format!("{id}.cs"),
        physical_path:format!("/{id}.cs"),logical_path:format!("{id}.cs"),provider_id:"capture".into(),provider_schema:"preclassified/1".into(),
        coordinate_unit:"utf16".into(),version:version.into(),source_digest:"opaque-fingerprint".into(),
        is_generated:false,is_excluded:false,facts:vec![],
        blocks:vec![durable::Block{block_id:"body".into(),span:durable::Span::offsets(10,31),tokens}] }
}
fn legacy_kind(k:durable::TokenKind)->legacy::CloneTokenKind {match k {
    durable::TokenKind::Other=>legacy::CloneTokenKind::Other,
    durable::TokenKind::LocalIdentifier=>legacy::CloneTokenKind::LocalIdentifier,
    durable::TokenKind::BoundIdentifier=>legacy::CloneTokenKind::BoundIdentifier,
    durable::TokenKind::Literal=>legacy::CloneTokenKind::Literal,
}}
fn legacy_doc(doc:&durable::Document)->legacy::CloneTokenDocument {
    legacy::CloneTokenDocument {document_id:doc.document_id.clone(),version:doc.version.clone(),
        is_generated:doc.is_generated,is_excluded:doc.is_excluded,blocks:doc.blocks.iter().map(|b| legacy::CloneTokenBlock {
            block_id:b.block_id.clone(),span:legacy::CloneSourceSpan {start_offset:b.span.start_offset,end_offset:b.span.end_offset},
            tokens:b.tokens.iter().map(|t|legacy::CloneToken{kind:legacy_kind(t.kind),text:t.text.clone()}).collect()
        }).collect() }
}
fn settings()->durable::Settings {durable::Settings{minimum_tokens:1,maximum_tokens_per_block:8,maximum_fingerprint_postings:4,..durable::Settings::default()}}
fn fence(rev:u64)->durable::Fence {durable::Fence{root_id:"root".into(),epoch:3,committed_revision:rev,input_digest:"a".repeat(64),tool_digest:"b".repeat(64),policy_digest:"c".repeat(64)}}
fn apply(index:&mut durable::FactIndex,rev:u64,docs:Vec<durable::Document>) {
    index.apply(durable::Batch {expected_revision:rev-1,fence:fence(rev),operations:docs.into_iter().map(|document|durable::Mutation::Replace{document}).collect()}).unwrap();
}
fn compare(index:&mut durable::FactIndex,model:&legacy::TokenCloneIndex,rev:u64,limit:legacy::CloneQueryLimits) {
    let our_limits=durable::QueryLimits{maximum_candidate_pairs:limit.maximum_candidate_pairs as u32,
        maximum_output_pairs:limit.maximum_output_pairs as u32,maximum_compared_tokens:limit.maximum_compared_tokens as u64,
        ..durable::QueryLimits::default()};
    let actual=index.query(&fence(rev),our_limits).unwrap();
    let expected=model.query(limit).unwrap();
    assert_eq!(actual.normalization_rule_version,legacy::RULE_VERSION);
    assert_eq!(actual.pairs.len(),expected.pairs.len());
    for (a,b) in actual.pairs.iter().zip(expected.pairs.iter()) {
        assert_eq!(a.pair_id,b.pair_id);
        assert_eq!(format!("{:?}",a.match_class),format!("{:?}",b.match_class));
        assert_eq!(a.normalization_rule_version,b.normalization_rule_version);
        assert_eq!(a.left.document_id,b.left.document_id);
        assert_eq!(a.left.document_version,b.left.document_version);
        assert_eq!(a.left.block_id,b.left.block_id);
        assert_eq!(a.left.span.start_offset,b.left.span.start_offset);
        assert_eq!(a.left.span.end_offset,b.left.span.end_offset);
        assert_eq!(a.right.document_id,b.right.document_id);
        assert_eq!(a.right.document_version,b.right.document_version);
        assert_eq!(a.right.block_id,b.right.block_id);
        assert_eq!(a.exact_matched_tokens as usize,b.exact_matched_tokens);
        assert_eq!(a.normalized_matched_tokens as usize,b.normalized_matched_tokens);
        assert_eq!(a.score,b.score);
    }
    assert_eq!(actual.gaps.len(),expected.gaps.len());
    for (a,b) in actual.gaps.iter().zip(expected.gaps.iter()) {
        assert_eq!(format!("{:?}",a.reason),format!("{:?}",b.reason));
        assert_eq!(a.scope_key,b.scope_key);
        assert_eq!(a.affected_count as i64,b.affected_count);
    }
    assert_eq!(actual.statistics.document_count as usize,expected.statistics.document_count);
    assert_eq!(actual.statistics.indexed_block_count as usize,expected.statistics.indexed_block_count);
    assert_eq!(actual.statistics.generated_documents as usize,expected.statistics.generated_documents);
    assert_eq!(actual.statistics.excluded_documents as usize,expected.statistics.excluded_documents);
    assert_eq!(actual.statistics.below_minimum_blocks as usize,expected.statistics.below_minimum_blocks);
    assert_eq!(actual.statistics.oversized_blocks as usize,expected.statistics.oversized_blocks);
    assert_eq!(actual.statistics.candidate_pairs_examined as usize,expected.statistics.candidate_pairs_examined);
    assert_eq!(actual.statistics.compared_tokens_charged as i64,expected.statistics.compared_tokens_charged);
}
fn limits(candidates:usize,output:usize,compared:i64)->legacy::CloneQueryLimits {
    legacy::CloneQueryLimits{maximum_candidate_pairs:candidates,maximum_output_pairs:output,maximum_compared_tokens:compared}
}
#[test]
fn exact_renamed_utf16_order_gaps_budgets_edits_deletes_and_cold_rebuild() {
    let scratch=Scratch::new();
    let mut index=durable::FactIndex::open(scratch.db(),"root",settings()).unwrap();
    let mut model=legacy::TokenCloneIndex::new(legacy::CloneIndexSettings{
        minimum_tokens:1,maximum_tokens_per_block:8,maximum_fingerprint_postings:4}).unwrap();
    let mut docs=vec![
        doc("alpha","v4",seq("x")),doc("beta","v1",seq("x")),doc("gamma","v1",seq("y")),
        doc("𐐀","v1",seq("u")),doc("\u{E000}","v1",seq("u")),
        doc("oversized","v1",vec![t(durable::TokenKind::Other,"X");9]),
        doc("below","v1",vec![]),
        doc("generated","v1",seq("x")),doc("excluded","v1",seq("x")),
    ];
    docs[7].is_generated=true; docs[8].is_excluded=true;
    for d in &docs {model.upsert(legacy_doc(d)).unwrap();}
    apply(&mut index,1,docs.clone());
    for lim in [limits(100,100,10_000),limits(1,100,10_000),limits(100,1,10_000),limits(100,100,11)] {
        compare(&mut index,&model,1,lim);
    }
    drop(index);
    let mut reopened=durable::FactIndex::open(scratch.db(),"root",settings()).unwrap();
    compare(&mut reopened,&model,1,limits(100,100,10_000));
    let replacement=doc("beta","v2",vec![t(durable::TokenKind::Literal,"changed")]);
    model.upsert(legacy_doc(&replacement)).unwrap();
    apply(&mut reopened,2,vec![replacement.clone()]);
    compare(&mut reopened,&model,2,limits(100,100,10_000));
    model.delete("alpha").unwrap();
    reopened.apply(durable::Batch{expected_revision:2,fence:fence(3),operations:vec![durable::Mutation::Delete{document_id:"alpha".into()}]}).unwrap();
    compare(&mut reopened,&model,3,limits(100,100,10_000));
    // A second, fresh in-memory index must agree with persisted warm/cold state.
    let mut cold=legacy::TokenCloneIndex::new(legacy::CloneIndexSettings{
        minimum_tokens:1,maximum_tokens_per_block:8,maximum_fingerprint_postings:4}).unwrap();
    for d in docs.iter().filter(|d|d.document_id!="alpha"&&d.document_id!="beta").rev() {cold.upsert(legacy_doc(d)).unwrap();}
    cold.upsert(legacy_doc(&replacement)).unwrap();
    compare(&mut reopened,&cold,3,limits(100,100,10_000));
}
