//! Source-only regression fixtures: run by the integration owner, not the recipient.
//! These use a real on-disk native SQLite store, not an in-memory mock.

use sha2::{Digest, Sha256};
use slophawk_test_control::*;
use slophawk_test_impact as impact;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(1);
fn fingerprint(value: &str) -> String { format!("{:X}", Sha256::digest(value.as_bytes())) }

struct Fixture { directory: PathBuf, path: PathBuf }
impl Fixture {
    fn new() -> Self {
        let name = format!("slophawk-sg-mig-11-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed));
        let directory=std::env::temp_dir().join(name);
        std::fs::create_dir(&directory).expect("private temporary directory");
        let path=directory.join("test-control-v1.sqlite3");
        Self { directory, path }
    }
    fn open(&self) -> Store { Store::open(&self.path).expect("open native durable store") }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.directory); }
}

fn fence(epoch: &str, generation: i64, sequence: i64, revision: i64) -> Fence {
    Fence { root:"physical-checkout-1".into(),context:"net10.0-debug".into(),
        epoch:epoch.into(),generation,sequence,revision,
        inputs:fingerprint(&format!("input-{revision}")),tool:fingerprint("dotnet-sdk-10"),
        policy:fingerprint("policy"),profile:fingerprint("profile-test"),
        grant:"trusted-capability".into(),budget:"bounded-v1".into() }
}
fn cause(revision: i64) -> Cause {
    Cause { revision,path_key:"Src/A.cs".into(),old_path:None,old_digest:None,
        new_path:Some("Src/A.cs".into()),new_digest:Some(fingerprint("class A {}")),
        predecessor:None,fallback_first_revision:None }
}
fn old_cause(revision: i64) -> Cause {
    Cause { revision,path_key:"Src/Deleted.cs".into(),old_path:Some("Src/Deleted.cs".into()),
        old_digest:Some(fingerprint("old")),new_path:None,new_digest:None,
        predecessor:Some("graph:missing".into()),fallback_first_revision:None }
}
fn graph(targets: usize, partial: bool) -> impact::ProjectGraph {
    impact::ProjectGraph {
        targets:(0..targets).map(|i| impact::ProjectTarget {
            id:format!("Tests/T{i}.csproj|net10.0"),
            project_path:format!("Tests/T{i}.csproj"),framework:"net10.0".into(),
            configuration:"Debug".into(),platform:"AnyCPU".into(),runtime_identifier:String::new(),
            sdk_version:"10.0.201".into(),is_test_project:true,
            compile:vec![impact::CompileMember{path:"Src/A.cs".into(),link:None}],
            references:vec![],evaluation_fingerprint:fingerprint(&format!("target-{i}"))
        }).collect(),
        issues:if partial {vec![impact::ProjectEvaluationIssue{
            path:".".into(),reason:"provider_frontier".into()}]} else {vec![]}
    }
}
fn selection_input(pending: &[Cause], targets: usize, partial: bool) -> impact::SelectionInput {
    let graph=graph(targets,partial);
    let approved_unit_projects=graph.targets.iter().map(|t|t.project_path.clone()).collect();
    impact::SelectionInput { current:graph,predecessors:HashMap::new(),
        source_digests:HashMap::from([("Src/A.cs".into(),fingerprint("class A {}"))]),
        approved_unit_projects, causes:pending.iter().map(Cause::to_impact).collect(),
        path_comparison:impact::PathComparison::Ordinal }
}

#[derive(Default)]
struct TestAuthority { current:Cell<bool>, drained:RefCell<HashSet<String>>, invalid_artifact:Cell<bool> }
impl TestAuthority {
    fn new() -> Self { Self {current:Cell::new(true),..Self::default()} }
    fn drained_scope(&self, scope: &str) { self.drained.borrow_mut().insert(scope.into()); }
}
impl Authority for TestAuthority {
    fn current(&self, _: &Fence) -> Result<()> {
        if self.current.get() {Ok(())} else {Err(ControlError::Stale)}
    }
    fn drained(&self, scope: &str) -> Result<()> {
        if self.drained.borrow().contains(scope) {Ok(())}
        else {Err(ControlError::Blocked("scope not drained"))}
    }
    fn verify_receipt(&self, _: &JobIdentity, receipt: &RunReceipt) -> Result<()> {
        if self.invalid_artifact.get() || receipt.artifact_digest!=fingerprint("owned trx bytes") {
            Err(ControlError::Invalid("artifact bytes changed"))
        } else {Ok(())}
    }
}
fn ready(store: &mut Store, schedule: &Fence, now: i64, authority: &TestAuthority, scope: &str) -> Job {
    let (state,plan)=store.next(schedule,now).expect("read next");
    assert_eq!(state,Admission::AwaitingSelection);
    let plan=plan.expect("a selected target");
    let response=store.admit(schedule,&plan,scope,now,authority).expect("durable admission");
    let Admission::Ready(job)=response else {panic!("no admission")};
    job
}
fn result(job: &JobIdentity, id: &str, disposition: RunDisposition) -> Terminal {
    let failed=disposition==RunDisposition::Failed;
    Terminal { identity:job.clone(),status:TerminalStatus::Complete,observed:Some(disposition),
        reason:None,omissions:vec![],
        receipt:Some(RunReceipt { receipt_id:id.into(),runner:"VSTest/TRX".into(),
            disposition,runner_completed:true,artifact_complete:true,
            artifact_digest:fingerprint("owned trx bytes"),artifact_bytes:15,
            collected_ids_digest:fingerprint("one discovered test"),
            counters:Counters {total:1,passed:if failed {0} else {1},
                failed:if failed {1} else {0},skipped:0} }) }
}
fn recorded(store: &mut Store, job: &Job, authority: &TestAuthority) {
    authority.drained_scope(&job.identity.scope);
    store.confirm_drained(&job.identity,authority).expect("durable drainage");
}

#[test]
fn zero_startup_restart_and_metadata_only_do_not_invent_execution() {
    let fixture=Fixture::new();
    let f=fence("first",1,0,0);
    let mut store=fixture.open();
    assert_eq!(store.bind(&f,&[]).unwrap().newly_admitted_causes,0);
    assert_eq!(store.pending().unwrap().state,"idle");
    drop(store);
    let mut store=fixture.open();
    assert_eq!(store.pending().unwrap().state,"idle");
    let metadata=fence("first",1,1,1);
    store.bind(&metadata,&[]).unwrap();
    assert_eq!(store.next(&metadata,0).unwrap().0,Admission::Idle);
    assert!(store.recoveries().unwrap().is_empty());
}

#[test]
fn complete_empty_retires_but_partial_empty_retains_causes() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    let (_,retired)=store.select(&f,&selection_input(&[c.clone()],0,true),&auth).unwrap();
    assert!(retired.is_empty());
    assert_eq!(store.pending().unwrap().causes.len(),1);
    let (selected,retired)=store.select(&f,&selection_input(&[c.clone()],0,false),&auth).unwrap();
    assert!(selected.complete);
    assert!(selected.targets.is_empty());
    assert_eq!(retired,vec![c.id()]);
    assert!(store.pending().unwrap().causes.is_empty());
}

#[test]
fn missing_predecessor_falls_back_to_approved_tests_and_limit_is_partial() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=old_cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    let input=selection_input(&[c.clone()],17,false);
    let (selected,retired)=store.select(&f,&input,&auth).unwrap();
    assert_eq!(selected.targets.len(),16);
    assert!(!selected.complete);
    assert!(selected.projection.conservative_fallback);
    assert_eq!(selected.projection.budget_deferred_targets,1);
    assert!(retired.is_empty());
    let job=ready(&mut store,&f,0,&auth,"scope-partial");
    recorded(&mut store,&job,&auth);
    assert_eq!(store.finish(&result(&job.identity,"receipt-partial",RunDisposition::Passed),0,&auth)
        .unwrap().retired.len(),0);
    assert_eq!(store.pending().unwrap().causes.len(),1);
}

#[test]
fn partial_predecessor_empty_selection_retains_cause_across_reopen() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=old_cause(1);
    let auth=TestAuthority::new();
    let mut input=selection_input(&[c.clone()],1,false);
    input.predecessors.insert("graph:missing".into(),graph(1,true));
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();

    let (selection,retired)=store.select(&f,&input,&auth).unwrap();
    assert!(!selection.complete);
    assert!(selection.targets.is_empty());
    assert_eq!(selection.projection.unresolved_frontiers[0].reason,"provider_frontier");
    assert!(retired.is_empty());
    assert_eq!(store.causes().unwrap(),vec![c.clone()]);
    drop(store);

    let mut store=fixture.open();
    assert_eq!(store.causes().unwrap(),vec![c.clone()]);
    assert_eq!(store.pending().unwrap().state,"partial_selection");
    assert_eq!(store.next(&f,0).unwrap(),(Admission::AwaitingSelection,None));
    // A replacement, qualified complete snapshot establishes the negative control.
    input.predecessors.insert("graph:missing".into(),graph(1,false));
    let corrected=fence("first",1,2,1);
    assert!(store.bind(&corrected,&[]).unwrap().revoke_scopes.is_empty());
    let (selection,retired)=store.select(&corrected,&input,&auth).unwrap();
    assert!(selection.complete);
    assert!(selection.targets.is_empty());
    assert_eq!(retired,vec![c.id()]);
    assert!(store.causes().unwrap().is_empty());
}

#[test]
fn a_failed_test_is_a_completed_measurement_but_zero_skip_incomplete_unadmitted_are_not() {
    for disposition in [RunDisposition::ZeroDiscovered,RunDisposition::Skipped,
        RunDisposition::Incomplete,RunDisposition::Unadmitted] {
        let fixture=Fixture::new();
        let f=fence("first",1,1,1);
        let c=cause(1);
        let auth=TestAuthority::new();
        let mut store=fixture.open();
        store.bind(&f,&[c.clone()]).unwrap();
        store.select(&f,&selection_input(&[c],1,false),&auth).unwrap();
        let job=ready(&mut store,&f,0,&auth,"scope-zero");
        recorded(&mut store,&job,&auth);
        let invalid=result(&job.identity,"invalid-completion",disposition);
        assert!(store.finish(&invalid,0,&auth).is_err());
        assert_eq!(store.pending().unwrap().causes.len(),1);
    }
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    store.select(&f,&selection_input(&[c],1,false),&auth).unwrap();
    let job=ready(&mut store,&f,0,&auth,"scope-failed-tests");
    recorded(&mut store,&job,&auth);
    let finish=store.finish(&result(&job.identity,"failed-tests",RunDisposition::Failed),0,&auth).unwrap();
    assert_eq!(finish.retired.len(),1);
    assert!(store.pending().unwrap().causes.is_empty());
}

#[test]
fn interrupted_attempt_survives_actual_store_reopen_and_drain_before_retry() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    store.select(&f,&selection_input(&[c],1,false),&auth).unwrap();
    let job=ready(&mut store,&f,0,&auth,"scope-crashed");
    drop(store);
    let mut reopened=fixture.open();
    assert_eq!(reopened.recoveries().unwrap(),vec![job.identity.clone()]);
    assert_eq!(reopened.next(&f,0).unwrap().0,Admission::Recovery("scope-crashed".into()));
    auth.drained_scope("scope-crashed");
    reopened.confirm_drained(&job.identity,&auth).unwrap();
    let retry=ready(&mut reopened,&f,0,&auth,"scope-after-reopen");
    assert_eq!(retry.identity.number,2);
    assert_eq!(reopened.pending().unwrap().causes.len(),1);
}

#[test]
fn retries_exhaust_and_keep_obligation_without_spinning() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    store.select(&f,&selection_input(&[c],1,false),&auth).unwrap();
    for n in 1..=3 {
        let time=1000*n;
        let job=ready(&mut store,&f,time,&auth,&format!("scope-retry-{n}"));
        assert_eq!(job.identity.number,n);
        recorded(&mut store,&job,&auth);
        let incomplete=Terminal {identity:job.identity.clone(),status:TerminalStatus::Failed,
            observed:Some(RunDisposition::Incomplete),
            reason:Some("runner did not produce complete evidence".into()),omissions:vec![],receipt:None};
        let disposition=store.finish(&incomplete,time,&auth).unwrap();
        assert_eq!(disposition.state,if n==3 {"exhausted"} else {"retryable"});
        assert_eq!(store.pending().unwrap().causes.len(),1);
    }
    assert_eq!(store.next(&f,4000).unwrap().0,Admission::Exhausted);
}

#[test]
fn edited_fence_and_profile_prevent_old_attempt_or_receipt_publication() {
    let fixture=Fixture::new();
    let first=fence("first",1,1,1);
    let mut changed=fence("first",1,2,2);
    changed.profile=fingerprint("revoked grant profile");
    changed.inputs=fingerprint("changed while worker ran");
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&first,&[c.clone()]).unwrap();
    store.select(&first,&selection_input(&[c],1,false),&auth).unwrap();
    let job=ready(&mut store,&first,0,&auth,"scope-edited");
    assert_eq!(store.bind(&changed,&[cause(2)]).unwrap().revoke_scopes,vec!["scope-edited".to_string()]);
    assert_eq!(store.pending().unwrap().state,"waiting_recovery");
    auth.drained_scope("scope-edited");
    store.confirm_drained(&job.identity,&auth).unwrap();
    assert!(store.finish(&result(&job.identity,"stale",RunDisposition::Passed),0,&auth).is_err());
    assert_eq!(store.pending().unwrap().causes.len(),2);
    assert!(store.bind(&first,&[]).is_err()); // stale sequence cannot reacquire
    let prior_epoch=fence("different",1,3,2);
    assert!(store.bind(&prior_epoch,&[]).is_err()); // epoch requires a new generation
}

#[test]
fn receipt_reuse_artifact_mismatch_and_failed_sql_commit_cannot_retire() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    store.select(&f,&selection_input(&[c],1,false),&auth).unwrap();
    let job=ready(&mut store,&f,0,&auth,"scope-rollback");
    recorded(&mut store,&job,&auth);
    auth.invalid_artifact.set(true);
    let good=result(&job.identity,"unique-id",RunDisposition::Passed);
    assert!(store.finish(&good,0,&auth).is_err());
    auth.invalid_artifact.set(false);
    // Real SQLite trigger aborts exactly between receipt validation and attempt update.
    let injector=rusqlite::Connection::open(&fixture.path).unwrap();
    injector.execute_batch("CREATE TRIGGER abort_receipt BEFORE INSERT ON receipt
        BEGIN SELECT RAISE(ABORT, 'injected store fault'); END;").unwrap();
    assert!(store.finish(&good,0,&auth).is_err());
    assert_eq!(store.pending().unwrap().causes.len(),1);
    injector.execute_batch("DROP TRIGGER abort_receipt").unwrap();
    let successful=store.finish(&good,0,&auth).unwrap();
    assert_eq!(successful.retired.len(),1);
    assert!(store.finish(&good,0,&auth).is_err());
    // Reuse remains forbidden across a newly admitted attempt as well.
    let second=fence("first",1,2,2);
    store.bind(&second,&[cause(2)]).unwrap();
    store.select(&second,&selection_input(&[cause(2)],1,false),&auth).unwrap();
    let new_job=ready(&mut store,&second,0,&auth,"scope-second");
    recorded(&mut store,&new_job,&auth);
    let reused=result(&new_job.identity,"unique-id",RunDisposition::Passed);
    assert!(store.finish(&reused,0,&auth).is_err());
    assert_eq!(store.pending().unwrap().causes.len(),1);
}

#[test]
fn trusted_export_import_retains_pending_and_recovery_not_old_coverage() {
    let original=Fixture::new();
    let replacement=Fixture::new();
    let first=fence("incarnation-a",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=original.open();
    store.bind(&first,&[c.clone()]).unwrap();
    store.select(&first,&selection_input(&[c],1,false),&auth).unwrap();
    let admitted=ready(&mut store,&first,0,&auth,"scope-transferred");
    let exported=store.export().unwrap();
    let mut imported=replacement.open();
    imported.import(&exported,&first.root,&first.context).unwrap();
    assert_eq!(imported.pending().unwrap().state,"waiting_recovery");
    assert_eq!(imported.recoveries().unwrap(),vec![admitted.identity.clone()]);
    assert!(imported.import(&exported,&first.root,&first.context).is_err());
    let new_fence=fence("incarnation-b",2,0,1);
    imported.bind(&new_fence,&[]).unwrap();
    assert!(imported.next(&new_fence,0).unwrap().0==Admission::Recovery("scope-transferred".into()));
    auth.drained_scope("scope-transferred");
    imported.confirm_drained(&admitted.identity,&auth).unwrap();
    assert_eq!(imported.next(&new_fence,0).unwrap().0,Admission::AwaitingSelection);
    let pending=imported.causes().unwrap();
    imported.select(&new_fence,&selection_input(&pending,1,false),&auth).unwrap();
    let retry=ready(&mut imported,&new_fence,0,&auth,"scope-migrated");
    assert_eq!(retry.identity.number,2);
}

struct InspectingPeer { file:PathBuf, authority:Rc<TestAuthority>, invoked:usize, observed_durable:bool }
impl Executor for InspectingPeer {
    fn reserve_scope(&mut self, _: &Fence, _: &Plan) -> Result<String> {
        Ok(format!("supervisor-scope-{}",self.invoked+1))
    }
    fn execute(&mut self, job: &Job) -> Result<Terminal> {
        self.invoked+=1;
        let connection=rusqlite::Connection::open(&self.file)?;
        let state:String=connection.query_row("SELECT state FROM attempt WHERE id=?1",
            [job.identity.attempt],|r|r.get(0))?;
        self.observed_durable=state=="running";
        Ok(result(&job.identity,"real-durable-receipt",RunDisposition::Passed))
    }
    fn revoke_and_drain(&mut self, scope: &str) -> Result<()> {
        self.authority.drained_scope(scope); Ok(())
    }
    fn confirm_drained(&mut self, scope: &str) -> Result<()> {
        self.authority.drained_scope(scope); Ok(())
    }
}
#[test]
fn bounded_peer_cannot_execute_before_durable_admission_and_drains_before_commit() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);
    let auth=Rc::new(TestAuthority::new());
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    store.select(&f,&selection_input(&[c],1,false),auth.as_ref()).unwrap();
    let mut peer=InspectingPeer{file:fixture.path.clone(),authority:auth.clone(),invoked:0,observed_durable:false};
    assert!(matches!(drive_one(&mut store,&f,0,auth.as_ref(),&mut peer),Ok(Admission::Ready(_))));
    assert_eq!(peer.invoked,1);
    assert!(peer.observed_durable);
    assert!(store.pending().unwrap().causes.is_empty());
    drop(store);
    let mut reopened=fixture.open();
    assert_eq!(reopened.pending().unwrap().state,"idle");
}

#[test]
fn selection_scope_must_match_every_pending_cause_and_cannot_reuse_old_epoch() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);let old=old_cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone(),old.clone()]).unwrap();
    assert!(store.select(&f,&selection_input(&[c],1,false),&auth).is_err());
    assert!(store.select(&f,&selection_input(&[old],1,false),&auth).is_err());
    assert_eq!(store.pending().unwrap().causes.len(),2);
}

#[test]
fn verification_delegates_validated_typed_evidence_and_preserves_failed_run_outcome() {
    use slophawk_verification as v;
    let scope=v::VerificationScope {context_id:"context".into(),target_id:"unit".into(),
        symbol_id:"API".into(),behavior_id:"contract".into(),
        source_digest:fingerprint("source"),build_id:fingerprint("build")};
    let rule=v::PUBLIC_CONTRACT_RULE_ID;
    let request=v::typed::TypedVerificationRequest {
        policy:v::VerificationPolicy {version:"v1".into(),default_mode:v::VerificationPolicyMode::Required,
            rules:vec![v::VerificationRuleSetting {rule_id:rule.into(),
                mode:v::VerificationPolicyMode::Required,required_detectors:vec!["runner".into()]}]},
        changes:vec![v::typed::TypedVerificationChange {kind:v::typed::VerificationChangeKind::PublicContract,
            change:v::VerificationChange {id:"edit".into(),scope:scope.clone(),
                changed_range:"0..5".into(),reasons:vec!["modified".into()]}}],
        searches:vec![v::VerificationSearchFact {id:"search".into(),rule_id:rule.into(),
            detector_id:"runner".into(),scope:scope.clone(),complete:true,
            searched_targets:vec!["unit".into()],excluded_targets:vec![],frontier:vec![],budget_exhausted:false}],
        evidence:vec![v::VerificationEvidence {id:"observed-failure".into(),rule_id:rule.into(),
            detector_id:"runner".into(),scope,kind:v::VerificationEvidenceKind::ContractExecution,
            binding:v::VerificationBinding::Exact,valid_receipt:true,exact_target_mapping:true,
            counter_mapping:v::VerificationCounterMapping::None,hit_count:None,
            run_outcome:v::VerificationRunOutcome::Failed,executed_behavior:true,relevant_assertion:true}],
    };
    let evaluated=evaluate_verification(&request,None).unwrap();
    assert_eq!(evaluated.decisions.len(),1);
    assert_eq!(evaluated.decisions[0].state,v::VerificationState::Observed);
    assert_eq!(evaluated.decisions[0].run_outcomes,vec![v::VerificationRunOutcome::Failed]);
}

#[test]
fn exact_retirement_waits_for_every_selected_target_across_reopen() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    let (selected,retired)=store.select(&f,&selection_input(&[c],2,false),&auth).unwrap();
    assert!(selected.complete);
    assert_eq!(selected.targets.len(),2);
    assert!(retired.is_empty());
    let one=ready(&mut store,&f,0,&auth,"scope-target-one");
    recorded(&mut store,&one,&auth);
    assert!(store.finish(&result(&one.identity,"target-one",RunDisposition::Passed),0,&auth)
        .unwrap().retired.is_empty());
    drop(store);
    let mut store=fixture.open();
    let two=ready(&mut store,&f,0,&auth,"scope-target-two");
    assert_ne!(one.identity.plan.target,two.identity.plan.target);
    recorded(&mut store,&two,&auth);
    assert_eq!(store.finish(&result(&two.identity,"target-two",RunDisposition::Passed),0,&auth)
        .unwrap().retired.len(),1);
    assert!(store.causes().unwrap().is_empty());
}

#[test]
fn old_graph_and_new_graph_independently_select_affected_tests() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,2);
    let c=Cause {revision:2,path_key:"Src/Old.cs".into(),
        old_path:Some("Src/Old.cs".into()),old_digest:Some(fingerprint("old")),
        new_path:None,new_digest:None,predecessor:Some("graph:1".into()),
        fallback_first_revision:None};
    let auth=TestAuthority::new();
    let old_graph=impact::ProjectGraph {targets:vec![impact::ProjectTarget {
        id:"Tests/T0.csproj|net10.0".into(),project_path:"Tests/T0.csproj".into(),
        framework:"net10.0".into(),configuration:"Debug".into(),platform:"AnyCPU".into(),
        runtime_identifier:String::new(),sdk_version:"10.0.201".into(),is_test_project:true,
        compile:vec![impact::CompileMember {path:"Src/Old.cs".into(),link:None}],
        references:vec![],evaluation_fingerprint:fingerprint("old graph") }],issues:vec![]};
    let mut input=selection_input(&[c.clone()],1,false);
    input.predecessors.insert("graph:1".into(),old_graph);
    let mut store=fixture.open();
    store.bind(&f,&[c]).unwrap();
    let (selection,retired)=store.select(&f,&input,&auth).unwrap();
    assert!(selection.complete);
    assert_eq!(selection.targets.len(),1);
    assert!(!selection.projection.conservative_fallback);
    assert!(retired.is_empty());
    let reason=&selection.projection.targets[0].reasons[0];
    assert_eq!(reason.kind,impact::TestSelectionReasonKind::PredecessorMembership);
}

#[test]
fn noncomplete_platform_dispositions_remain_different_durable_observations() {
    for (status,observed) in [
        (TerminalStatus::Partial,RunDisposition::ZeroDiscovered),
        (TerminalStatus::Deferred,RunDisposition::Skipped),
        (TerminalStatus::Failed,RunDisposition::Incomplete),
        (TerminalStatus::Unsupported,RunDisposition::Unadmitted),
    ] {
        let fixture=Fixture::new();
        let f=fence("first",1,1,1);
        let c=cause(1);
        let auth=TestAuthority::new();
        let mut store=fixture.open();
        store.bind(&f,&[c.clone()]).unwrap();
        store.select(&f,&selection_input(&[c],1,false),&auth).unwrap();
        let job=ready(&mut store,&f,0,&auth,"scope-noncomplete");
        recorded(&mut store,&job,&auth);
        let terminal=Terminal {identity:job.identity.clone(),status,observed:Some(observed),
            reason:Some("bounded diagnostic".into()),
            omissions:if status==TerminalStatus::Partial {vec!["unobserved case".into()]} else {vec![]},
            receipt:None};
        store.finish(&terminal,0,&auth).unwrap();
        drop(store);
        let mut reopened=fixture.open();
        let p=reopened.pending().unwrap();
        assert_eq!(p.causes.len(),1);
        let latest=p.latest.expect("persisted disposition");
        assert_eq!(latest.status,status);
        assert_eq!(latest.disposition,Some(observed));
    }
}

#[test]
fn source_transaction_failure_reopens_without_a_partial_revision_or_intent() {
    let fixture=Fixture::new();
    let f=fence("first",1,1,1);
    let next=fence("first",1,2,2);
    let mut store=fixture.open();
    store.bind(&f,&[cause(1)]).unwrap();
    let injector=rusqlite::Connection::open(&fixture.path).unwrap();
    injector.execute_batch("CREATE TRIGGER deny_intent BEFORE INSERT ON cause
        BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END;").unwrap();
    assert!(store.bind(&next,&[cause(2)]).is_err());
    injector.execute_batch("DROP TRIGGER deny_intent").unwrap();
    drop(store);
    let mut reopened=fixture.open();
    assert_eq!(reopened.fence().unwrap(),Some(f));
    assert_eq!(reopened.pending().unwrap().causes.len(),1);
    reopened.bind(&next,&[cause(2)]).unwrap();
    assert_eq!(reopened.pending().unwrap().causes.len(),2);
}

#[test]
fn imported_work_requires_new_writer_generation_even_with_same_source_digests() {
    let fixture=Fixture::new();
    let replacement=Fixture::new();
    let f=fence("initial",1,1,1);
    let mut store=fixture.open();
    store.bind(&f,&[cause(1)]).unwrap();
    let bytes=store.export().unwrap();
    let mut imported=replacement.open();
    imported.import(&bytes,&f.root,&f.context).unwrap();
    assert!(imported.next(&f,0).is_err());
    assert!(imported.bind(&f,&[]).is_err());
    imported.bind(&fence("fresh",2,0,1),&[]).unwrap();
    assert_eq!(imported.pending().unwrap().causes.len(),1);
}

#[test]
fn failed_admission_transaction_never_leaks_a_running_attempt_across_reopen() {
    let fixture=Fixture::new();
    let f=fence("initial",1,1,1);
    let c=cause(1);
    let auth=TestAuthority::new();
    let mut store=fixture.open();
    store.bind(&f,&[c.clone()]).unwrap();
    store.select(&f,&selection_input(&[c],1,false),&auth).unwrap();
    let (_,plan)=store.next(&f,0).unwrap();
    let plan=plan.unwrap();
    let injector=rusqlite::Connection::open(&fixture.path).unwrap();
    injector.execute_batch("CREATE TRIGGER refuse_admit BEFORE INSERT ON attempt
        BEGIN SELECT RAISE(ABORT, 'interleaved admission failure'); END;").unwrap();
    assert!(store.admit(&f,&plan,"scope-never-started",0,&auth).is_err());
    let count:i64=injector.query_row("SELECT count(*) FROM attempt",[],|r|r.get(0)).unwrap();
    assert_eq!(count,0);
    injector.execute_batch("DROP TRIGGER refuse_admit").unwrap();
    drop(injector);
    drop(store);
    let mut reopened=fixture.open();
    assert!(reopened.recoveries().unwrap().is_empty());
    assert_eq!(reopened.pending().unwrap().causes.len(),1);
    let job=ready(&mut reopened,&f,0,&auth,"scope-after-denied-commit");
    assert_eq!(job.identity.number,1);
}

#[test]
fn rebind_and_drain_revokes_exact_prior_scope_before_any_new_work() {
    let fixture=Fixture::new();
    let before=fence("before",1,1,1);
    let after=fence("before",1,2,2);
    let auth=Rc::new(TestAuthority::new());
    let c=cause(1);
    let mut store=fixture.open();
    store.bind(&before,&[c.clone()]).unwrap();
    store.select(&before,&selection_input(&[c],1,false),auth.as_ref()).unwrap();
    let admitted=ready(&mut store,&before,0,auth.as_ref(),"scope-revoked");
    let mut peer=InspectingPeer {file:fixture.path.clone(),authority:auth.clone(),invoked:0,observed_durable:false};
    let binding=store.bind_and_drain(&after,&[cause(2)],auth.as_ref(),&mut peer).unwrap();
    assert_eq!(binding.revoke_scopes,vec!["scope-revoked".to_string()]);
    assert!(store.recoveries().unwrap().is_empty());
    assert_eq!(store.next(&after,0).unwrap().0,Admission::AwaitingSelection);
    assert!(store.finish(&result(&admitted.identity,"too-late",RunDisposition::Passed),0,auth.as_ref()).is_err());
    assert_eq!(store.pending().unwrap().causes.len(),2);
}
