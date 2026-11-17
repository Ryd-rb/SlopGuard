use slophawk_fact_index::*;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64,Ordering};

static SERIAL: AtomicU64=AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new()->Self {
        let p=std::env::temp_dir().join(format!("slophawk-sg-mig-09-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir_all(&p).unwrap(); Self(p)
    }
    fn db(&self)->PathBuf {self.0.join("facts.sqlite")}
}
impl Drop for Scratch {fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
fn d(ch:char)->String {ch.to_string().repeat(64)}
fn fence(rev:u64)->Fence {Fence{root_id:"checkout-A".into(),epoch:7,committed_revision:rev,input_digest:d('a'),tool_digest:d('b'),policy_digest:d('c')}}
fn settings()->Settings {Settings{minimum_tokens:1,maximum_tokens_per_block:8,maximum_fingerprint_postings:4,..Settings::default()}}
fn t(kind:TokenKind,text:&str)->Token{Token{kind,text:text.into()}}
fn body(name:&str)->Vec<Token> {vec![t(TokenKind::Other,"let"),t(TokenKind::LocalIdentifier,name),t(TokenKind::Other,"="),t(TokenKind::LocalIdentifier,name)]}
fn doc(id:&str,version:&str,tokens:Vec<Token>)->Document {
    Document{document_id:id.into(),context_key:"target/net10.0/Debug".into(),source_id:format!("{id}.cs"),
        physical_path:format!("/checkouts/a/{id}.cs"),logical_path:format!("{id}.cs"),provider_id:"test.preclassified".into(),
        provider_schema:"test/1".into(),coordinate_unit:"utf16".into(),version:version.into(),source_digest:"physical-digest-opaque".into(),
        is_generated:false,is_excluded:false,facts:vec![Fact{kind:"declaration".into(),fact_id:"fact-0".into(),
            symbol_id:Some("symbol:Foo".into()),owner_id:None,span:Some(Span{start_offset:1,end_offset:4,
                start_line:Some(2),start_column:Some(1),end_line:Some(2),end_column:Some(4)}),payload:serde_json::json!({"Name":"Foo","ProviderId":"test"})}],
        blocks:vec![Block{block_id:"body".into(),span:Span{start_offset:5,end_offset:24,
            start_line:Some(2),start_column:Some(0),end_line:Some(3),end_column:Some(4)},tokens}]}
}
fn apply(index:&mut FactIndex,rev:u64,operations:Vec<Mutation>)->CommitReceipt {
    index.apply(Batch{expected_revision:rev-1,fence:fence(rev),operations}).unwrap()
}
fn query(index:&mut FactIndex,rev:u64)->QueryResult{index.query(&fence(rev),QueryLimits::default()).unwrap()}

#[test]
fn atomic_lifecycle_rename_invalidation_delete_reopen_and_spans() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    assert_eq!(index.state().unwrap().committed_revision,0);
    apply(&mut index,1,vec![
        Mutation::Replace{document:doc("a","v1",body("x"))},
        Mutation::Replace{document:doc("b","v1",body("x"))},
        Mutation::Replace{document:doc("c","v1",body("y"))},
    ]);
    let initial=query(&mut index,1);
    assert_eq!(initial.pairs.len(),3);
    assert_eq!(initial.pairs[0].match_class,MatchClass::Exact);
    assert_eq!(initial.pairs[0].pair_id,"sg10.token-clones.v1:1:a4:body/1:b4:body");
    assert_eq!(initial.pairs[1].match_class,MatchClass::RenamedLocalIdentifiers);
    assert_eq!(initial.pairs[0].left.context_key,"target/net10.0/Debug");
    assert_eq!(initial.pairs[0].left.provider_id,"test.preclassified");
    assert_eq!(initial.pairs[0].left.span.start_line,Some(2));
    let facts=index.facts_page(&fence(1),"a",0,3).unwrap();
    assert_eq!(facts.len(),1);
    assert_eq!(facts[0].span.unwrap().start_column,Some(1));
    assert_eq!(facts[0].payload["Name"],"Foo");
    let repeat=query(&mut index,1);
    assert_eq!(initial,repeat);
    drop(index);
    let mut reopened=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    let cold=query(&mut reopened,1);
    assert_eq!(initial,cold);
    apply(&mut reopened,2,vec![Mutation::Rename{document_id:"a".into(),new_document_id:"aa".into(),
        new_source_id:Some("aa.cs".into()),new_version:Some("v2".into()),
        new_logical_path:Some("aa.cs".into()),new_physical_path:Some("/checkouts/a/aa.cs".into()),
        new_context_key:None}]);
    let renamed=query(&mut reopened,2);
    assert_eq!(renamed.pairs.len(),3);
    assert!(renamed.pairs.iter().any(|p|p.pair_id.contains("2:aa4:body")));
    assert!(renamed.pairs.iter().all(|p|!p.pair_id.contains("1:a4:body")));
    assert_eq!(reopened.facts_page(&fence(2),"aa",0,8).unwrap().len(),1);
    assert!(matches!(reopened.query(&fence(1),QueryLimits::default()),Err(Error::Stale(_))));
    apply(&mut reopened,3,vec![Mutation::Replace{document:doc("b","v3",vec![t(TokenKind::Literal,"different")])}]);
    assert_eq!(query(&mut reopened,3).pairs.len(),1); // aa/c; b was atomically withdrawn
    apply(&mut reopened,4,vec![Mutation::Invalidate{document_id:"c".into()}]);
    assert_eq!(query(&mut reopened,4).pairs.len(),0);
    assert!(reopened.facts_page(&fence(4),"c",0,5).unwrap().is_empty());
    apply(&mut reopened,5,vec![Mutation::Delete{document_id:"aa".into()}]);
    assert_eq!(query(&mut reopened,5).statistics.document_count,2); // b and invalid c
    drop(reopened);
    let mut last=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    assert_eq!(last.state().unwrap(),fence(5));
    assert!(query(&mut last,5).pairs.is_empty());
    assert_eq!(last.facts_page(&fence(5),"aa",0,5).unwrap().len(),0);
}

#[test]
fn invalid_batch_or_conflicting_rename_rollback_all_postings_and_fence() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    apply(&mut index,1,vec![Mutation::Replace{document:doc("a","v1",body("x"))},Mutation::Replace{document:doc("b","v1",body("x"))}]);
    let before=query(&mut index,1);
    let mut malformed=doc("a","bad",body("x"));
    malformed.blocks[0].tokens[0].text.clear();
    let error=index.apply(Batch{expected_revision:1,fence:fence(2),operations:vec![
        Mutation::Delete{document_id:"b".into()},Mutation::Replace{document:malformed}]}).unwrap_err();
    assert!(matches!(error,Error::Budget(_)));
    assert_eq!(before,query(&mut index,1));
    assert!(index.apply(Batch{expected_revision:1,fence:fence(2),operations:vec![
        Mutation::Delete{document_id:"b".into()},Mutation::Rename{document_id:"a".into(),new_document_id:"a".into(),new_source_id:None,new_version:None,
            new_logical_path:None,new_physical_path:None,new_context_key:None},
        Mutation::Invalidate{document_id:"not-there".into()}]}).is_err());
    assert_eq!(before,query(&mut index,1));
    let mut stale=fence(2); stale.epoch=6;
    assert!(matches!(index.apply(Batch{expected_revision:1,fence:stale,operations:vec![Mutation::Delete{document_id:"a".into()}]}),Err(Error::Stale(_))));
    assert_eq!(before,query(&mut index,1));
}

#[test]
fn sqlite_rollback_after_interrupted_transaction_preserves_revision() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    apply(&mut index,1,vec![Mutation::Replace{document:doc("a","v1",body("x"))}]);
    drop(index);
    {
        let mut direct=rusqlite::Connection::open(scratch.db()).unwrap();
        let transaction=direct.transaction().unwrap();
        transaction.execute("UPDATE workspace SET revision=999 WHERE id=1",[]).unwrap();
        transaction.execute("DELETE FROM documents",[]).unwrap();
        // Process death before COMMIT: Drop rolls back SQLite's transaction.
    }
    let mut reopened=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    assert_eq!(reopened.state().unwrap(),fence(1));
    assert_eq!(query(&mut reopened,1).statistics.document_count,1);
}

#[test]
fn incompatible_schema_corrupted_posting_and_foreign_root_fail_explicitly() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    apply(&mut index,1,vec![Mutation::Replace{document:doc("a","v1",body("x"))},Mutation::Replace{document:doc("b","v1",body("x"))}]);
    drop(index);
    assert!(matches!(FactIndex::open(scratch.db(),"wrong-root",settings()),Err(Error::Stale(_))));
    assert!(matches!(FactIndex::open(scratch.db(),"checkout-A",Settings{minimum_tokens:2,..settings()}),Err(Error::UnsupportedFormat(_))));
    {
        let direct=rusqlite::Connection::open(scratch.db()).unwrap();
        direct.execute("UPDATE postings SET fingerprint=zeroblob(32) WHERE document_id='b' AND pass=0",[]).unwrap();
    }
    let mut reopened=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    assert!(matches!(reopened.query(&fence(1),QueryLimits::default()),Err(Error::Corrupt(_))));
    drop(reopened);
    {
        let direct=rusqlite::Connection::open(scratch.db()).unwrap();
        direct.pragma_update(None,"user_version",99).unwrap();
    }
    assert!(matches!(FactIndex::open(scratch.db(),"checkout-A",settings()),Err(Error::UnsupportedFormat(_))));
    let garbage=scratch.0.join("random.db");
    fs::write(&garbage,b"not sqlite").unwrap();
    assert!(FactIndex::open(&garbage,"checkout-A",settings()).is_err());
}

#[test]
fn fanout_oversized_generated_excluded_and_query_budget_frontiers_are_explicit() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",Settings{maximum_fingerprint_postings:2,..settings()}).unwrap();
    let mut oversized=doc("big","v1",vec![t(TokenKind::Other,"X");9]);
    let mut generated=doc("gen","v1",body("x")); generated.is_generated=true;
    let mut excluded=doc("exc","v1",body("x")); excluded.is_excluded=true;
    oversized.facts.clear();
    apply(&mut index,1,vec![
        Mutation::Replace{document:doc("a","v1",body("x"))},
        Mutation::Replace{document:doc("b","v1",body("x"))},
        Mutation::Replace{document:doc("c","v1",body("x"))},
        Mutation::Replace{document:oversized},Mutation::Replace{document:generated},Mutation::Replace{document:excluded},
    ]);
    let result=query(&mut index,1);
    assert_eq!(result.statistics.indexed_block_count,3);
    assert_eq!(result.statistics.generated_documents,1);
    assert_eq!(result.statistics.excluded_documents,1);
    assert_eq!(result.statistics.oversized_blocks,1);
    assert!(result.pairs.is_empty());
    assert_eq!(result.gaps.iter().filter(|g|g.reason==GapReason::BoilerplatePostingLimit).count(),2);
    assert!(result.gaps.iter().any(|g|g.reason==GapReason::OversizedBlock&&g.affected_count==9));
    let limited=index.query(&fence(1),QueryLimits{maximum_scanned_blocks:1,..QueryLimits::default()}).unwrap();
    assert!(limited.gaps.iter().any(|g|g.reason==GapReason::ScannedBlockLimit));
}

#[test]
fn captured_roslyn_v1_preserves_provider_facts_and_contexts() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    let capture=|context:&str|serde_json::json!({
        "SchemaVersion":1,
        "Identity":{"ContextKey":context,"DocumentKey":"shared.cs","PhysicalPath":"/shared.cs",
                    "LogicalPath":"shared.cs","Revision":"r1","InputFingerprint":"provider-opaque-fingerprint",
                    "ReferenceClosureComplete":false},
        "CompilationAssembly":"Example, Version=1.0.0.0","RoslynCommonAssemblyVersion":"5.0.0.0",
        "RoslynCSharpAssemblyVersion":"5.0.0.0",
        "Declarations":[{"SymbolId":"sym:one","Name":"Method","SymbolKind":"Method",
            "Span":{"Start":2,"Length":4,"StartLine":1,"StartColumn":2,"EndLine":1,"EndColumn":6}}],
        "References":[],"Calls":[],"Frontier":[{"Reason":"MissingCompilationReferences","Span":null,"Detail":"not certified"}]
    });
    let a=Document::from_roslyn_json(&capture("ctx-a")).unwrap();
    let b=Document::from_roslyn_json(&capture("ctx-b")).unwrap();
    assert_ne!(a.document_id,b.document_id);
    let aid=a.document_id.clone();
    apply(&mut index,1,vec![Mutation::Replace{document:a},Mutation::CaptureRoslyn{facts:capture("ctx-b")}]);
    let records=index.facts_page(&fence(1),&aid,0,8).unwrap();
    assert_eq!(records.len(),2);
    assert_eq!(records[0].span.unwrap().start_offset,2);
    assert_eq!(records[0].payload["Name"],"Method");
    assert_eq!(records[1].kind,"frontier");
    assert_eq!(query(&mut index,1).statistics.document_count,2);
    let mut bad=capture("ctx-a"); bad["SchemaVersion"]=serde_json::json!(2);
    assert!(matches!(Document::from_roslyn_json(&bad),Err(Error::UnsupportedFormat(_))));
    drop(index);
    let mut reopened=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    assert_eq!(reopened.facts_page(&fence(1),&aid,0,8).unwrap().len(),2);
}

#[test]
fn bounded_fact_pages_and_query_bytes_expire_on_next_revision() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    let mut a=doc("a","v1",body("x"));
    for i in 1..70 {
        a.facts.push(Fact {kind:"declaration".into(),fact_id:format!("fact-{i}"),
            symbol_id:Some(format!("symbol:{i}")),owner_id:None,span:Some(Span::offsets(i,i+1)),
            payload:serde_json::json!({"index":i})});
    }
    apply(&mut index,1,vec![Mutation::Replace{document:a},Mutation::Replace{document:doc("b","v1",body("x"))}]);
    assert_eq!(index.facts_page(&fence(1),"a",0,64).unwrap().len(),64);
    let last=index.facts_page(&fence(1),"a",64,64).unwrap();
    assert_eq!(last.len(),6);
    assert_eq!(last[0].fact_id,"fact-64");
    assert!(matches!(index.facts_page(&fence(1),"a",0,129),Err(Error::Budget(_))));
    let output=index.query(&fence(1),QueryLimits{maximum_output_bytes:1024,..QueryLimits::default()}).unwrap();
    assert!(output.pairs.is_empty());
    assert!(output.gaps.iter().any(|g|g.reason==GapReason::OutputBytesLimit));
    apply(&mut index,2,vec![Mutation::Delete{document_id:"b".into()}]);
    assert!(matches!(index.facts_page(&fence(1),"a",64,64),Err(Error::Stale(_))));
    assert_eq!(index.facts_page(&fence(2),"a",64,64).unwrap().len(),6);
}

#[test]
fn forged_fingerprint_collision_does_not_promote_a_false_match() {
    let scratch=Scratch::new();
    let mut index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    apply(&mut index,1,vec![Mutation::Replace{document:doc("a","v1",body("x"))},
        Mutation::Replace{document:doc("b","v1",body("y"))}]);
    drop(index);
    {
        let direct=rusqlite::Connection::open(scratch.db()).unwrap();
        let fp:Vec<u8>=direct.query_row("SELECT exact_fp FROM blocks WHERE document_id='a'",[],|r|r.get(0)).unwrap();
        let old:Vec<u8>=direct.query_row("SELECT exact_fp FROM blocks WHERE document_id='b'",[],|r|r.get(0)).unwrap();
        direct.execute("UPDATE blocks SET exact_fp=?1 WHERE document_id='b'",[fp.as_slice()]).unwrap();
        direct.execute("UPDATE postings SET fingerprint=?1 WHERE pass=0 AND document_id='b'",[fp.as_slice()]).unwrap();
        direct.execute("UPDATE bucket_counts SET posting_count=2 WHERE pass=0 AND fingerprint=?1",[fp.as_slice()]).unwrap();
        direct.execute("DELETE FROM bucket_counts WHERE pass=0 AND fingerprint=?1",[old.as_slice()]).unwrap();
    }
    let mut reopened=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    assert!(matches!(reopened.query(&fence(1),QueryLimits::default()),Err(Error::Corrupt(_))));
}

#[test]
fn altered_schema_sql_is_rejected_even_with_original_object_names() {
    let scratch=Scratch::new();
    let index=FactIndex::open(scratch.db(),"checkout-A",settings()).unwrap();
    drop(index);
    let direct=rusqlite::Connection::open(scratch.db()).unwrap();
    direct.execute_batch("DROP TRIGGER postings_deleted;
        CREATE TRIGGER postings_deleted AFTER DELETE ON postings BEGIN SELECT 1; END;").unwrap();
    drop(direct);
    assert!(matches!(FactIndex::open(scratch.db(),"checkout-A",settings()),Err(Error::UnsupportedFormat(_))));
}
