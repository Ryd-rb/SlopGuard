//! Differential contract cases follow the pinned C# `BuildOutputIdentityTests`,
//! `CoverageReceiptCaptureTests`, `CoberturaCoverageImporterTests` and
//! `TestRunReceiptDetails` validation matrix. Not executed by the recipient.
use slophawk_receipts_coverage::{identity as build, receipt as test,
    cobertura as cov, capture, sha256_hex};

const A:&str="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const B:&str="BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const C:&str="CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
const EPOCH:&str="da8a8249-76b2-401e-a156-bce5db02cd9c";

fn inputs()->build::BuildInputs {
    build::BuildInputs {
        target:build::ProjectTarget {id:Some("tests/Unit.csproj|net10.0".into()),
            project_path:Some("tests/Unit.csproj".into()),framework:Some("net10.0".into()),
            configuration:Some("Debug".into()),platform:Some("AnyCPU".into()),
            runtime_identifier:Some("".into()),sdk_version:Some("10.0.201".into()),
            is_test_project:true,evaluation_fingerprint:Some(A.into()),
            compile:Some(vec![build::Member{path:"tests/Alpha.cs".into(),link:None},
                build::Member{path:"tests/Beta.cs".into(),link:Some("Shared/Beta.cs".into())}]),
            references:Some(vec!["lib/Lib.csproj|net10.0".into()])},
        project_graph_complete:true,path_mapping_identity:Some("trusted-physical-map".into()),
        path_mapping_trusted:true,collection_mode:build::CollectionMode::ImmutableSnapshot,
        settings_sha256:Some(A.into()),runner_identity:Some("vstest-1".into()),
        adapter_identity:Some("adapter/123".into()),
        ordered_compile_inputs:Some(vec![
            build::CompileInput {path:"tests/Alpha.cs".into(),link:None,sha256:Some(B.into())},
            build::CompileInput {path:"tests/Beta.cs".into(),link:Some("Shared/Beta.cs".into()),sha256:Some(C.into())}]),
        reference_assemblies:Some(vec![build::IdentityFile{path:"ref/Lib.dll".into(),sha256:Some(B.into())}]),
        runtime_assets:Some(vec![build::IdentityFile{path:"assets/config.json".into(),sha256:Some(C.into())}]),
        declared_outputs:Some(vec![build::DeclaredOutput{path:"out/Unit.dll".into(),
            sha256:Some(A.into()),observation:build::OutputObservation::ByteVerified}]),
    }
}
#[test]
fn identity_sections_and_complete_reuse_match_csharp_contract() {
    let before=build::capture(&inputs());
    assert!(before.complete); assert_eq!(11,before.sections.len());
    assert_eq!(64,before.sha256.len());
    assert!(build::compare(&before,&before).reusable);
    let mut moved=inputs();
    moved.reference_assemblies.as_mut().unwrap()[0].path="ref\\Lib.dll".into();
    moved.runtime_assets.as_mut().unwrap()[0].sha256=Some(C.to_ascii_lowercase());
    assert_eq!(before.sha256,build::capture(&moved).sha256);
}
#[test]
fn changed_runtime_or_ordered_compilation_invalidates_reuse() {
    let old=build::capture(&inputs());
    let mut changed=inputs(); changed.runtime_assets.as_mut().unwrap()[0].sha256=Some(B.into());
    assert!(build::compare(&old,&build::capture(&changed)).problems.iter()
        .any(|p|p.field=="runtime-assets"));
    let mut order=inputs();order.ordered_compile_inputs.as_mut().unwrap().reverse();
    assert!(build::compare(&old,&build::capture(&order)).problems.iter()
        .any(|p|p.field=="compile"));
}
#[test]
fn gaps_wrong_hashes_and_tampered_sections_are_never_reusable() {
    let mut missing=inputs();missing.runtime_assets=None;
    let stamp=build::capture(&missing);assert!(!stamp.complete);
    assert!(!build::compare(&stamp,&stamp).reusable);
    let mut bad=inputs();bad.declared_outputs.as_mut().unwrap()[0].sha256=Some("oops".into());
    assert!(!build::capture(&bad).complete);
    let mut bad=inputs();bad.declared_outputs.as_mut().unwrap()[0].observation=build::OutputObservation::ExpectedOnly;
    assert!(!build::capture(&bad).complete);
    let mut bad=inputs();bad.ordered_compile_inputs.as_mut().unwrap()[0].path="../escape.cs".into();
    assert!(!build::capture(&bad).complete);
    let good=build::capture(&inputs());
    let mut forged=good.clone();forged.sections[0].sha256=B.into();
    assert!(!build::compare(&good,&forged).reusable);
    let mut forged=good.clone();forged.domain_version=2;
    assert!(!build::compare(&good,&forged).reusable);
}
#[test]
fn closure_requires_every_observed_runtime_input_to_be_declared() {
    let stamp=build::capture(&inputs());
    let required=vec!["assets/config.json".into()];
    assert!(build::closure_checked(&stamp,&inputs(),&required,&required).reusable);
    assert!(!build::closure_checked(&stamp,&inputs(),&required,&vec!["secrets/undeclared.dat".into()]).reusable);
    assert!(!build::closure_checked(&stamp,&inputs(),&["different/input.dat".into()],&[]).reusable);
}

fn admitted()->test::AdmittedAttempt {
    test::AdmittedAttempt {attempt_id:42,revision:5,epoch:EPOCH.into(),fence_sequence:17,
        git_branch:"main".into(),git_commit:"a367ef".into(),root_identity:"root-1".into(),
        target:"unit".into(),input_fingerprint:"inputs".into(),profile_revision:"profile".into(),
        stored_lease_id:Some("lease".into()),scheduler:test::Scheduler::Failed}
}
fn details()->test::TestReceipt {
    test::TestReceipt {format_version:1,attempt_id:42,revision:5,epoch:EPOCH.into(),fence_sequence:17,
        git_fingerprint:test::git_identity("main","a367ef"),root_identity:"root-1".into(),
        target:"unit".into(),input_fingerprint:"inputs".into(),profile_revision:"profile".into(),
        lease_id:Some("lease".into()),build_domain_version:Some(1),build_sha256:Some(A.into()),
        runner_sha256:Some(B.into()),artifact_sha256:Some(C.into()),artifact_bytes:Some(100),
        run_id:Some("00000000-0000-0000-0000-000000000001".into()),
        platform:test::Platform::Failed,runner:test::Runner::NonzeroExit,
        artifact:test::Artifact::Complete,execution:test::Execution::Failed,
        counters:Some(test::Counters{total:2,executed:2,passed:1,failed:1,skipped:0,other:0}),
        reason_code:"failed_tests".into()}
}
#[test]
fn receipt_requires_attempt_identity_and_fails_closed_on_bad_hash() {
    assert!(test::validate(&details(),&admitted()).is_ok());
    assert!(test::usable_for_coverage(&details(),&admitted()));
    let mut altered=admitted();altered.fence_sequence+=1;
    assert!(test::validate(&details(),&altered).is_err());
    let mut altered=details();altered.artifact_sha256=Some("wrong-hash".into());
    assert!(test::validate_payload(&altered).is_err());
    let mut altered=details();altered.format_version=99;
    assert_eq!(slophawk_receipts_coverage::Status::Unsupported,
        test::validate_payload(&altered).unwrap_err().status);
    let mut altered=details();altered.reason_code="unbounded details: path /tmp".into();
    assert!(test::validate_payload(&altered).is_err());
    let mut altered=details();altered.counters.as_mut().unwrap().failed=3;
    assert!(test::validate_payload(&altered).is_err());
}
#[test]
fn zero_skipped_failed_and_incomplete_are_not_reclassified_as_passed() {
    let mut zero=details();zero.platform=test::Platform::ZeroDiscovered;
    zero.execution=test::Execution::ZeroDiscovered;
    zero.counters=Some(test::Counters{total:0,executed:0,passed:0,failed:0,skipped:0,other:0});
    assert!(test::validate_payload(&zero).is_ok());
    assert!(!test::usable_for_coverage(&zero,&admitted()));
    let mut skipped=details();skipped.platform=test::Platform::AllSkipped;
    skipped.execution=test::Execution::AllSkipped;
    skipped.counters=Some(test::Counters{total:2,executed:0,passed:0,failed:0,skipped:2,other:0});
    assert!(test::validate_payload(&skipped).is_ok());
    let mut incomplete=details();incomplete.platform=test::Platform::Incomplete;
    incomplete.runner=test::Runner::Incomplete;incomplete.execution=test::Execution::Unknown;
    incomplete.artifact=test::Artifact::Missing;incomplete.counters=None;
    assert!(test::validate_payload(&incomplete).is_ok());
    let mut passed=details();passed.platform=test::Platform::Passed;
    passed.execution=test::Execution::Passed;passed.runner=test::Runner::ZeroExit;
    passed.counters=Some(test::Counters{total:2,executed:2,passed:2,failed:0,skipped:0,other:0});
    assert!(test::validate_payload(&passed).is_ok());
    assert!(test::validate(&passed,&admitted()).is_err()); // admitted scheduler was failed
    assert!(test::validate(&details(),&admitted()).is_ok()); // a failed run remains failed
}

fn xml()->String {
    "<coverage lines-covered='0' lines-valid='1' branches-covered='1' branches-valid='2'>\
      <packages><package name='unit'><classes><class name='A' filename='src/A.cs'>\
      <methods><method name='M' signature='()'><lines><line number='12' hits='99'/></lines></method></methods>\
      <lines><line number='12' hits='0' branch='true' condition-coverage='50% (1/2)'/></lines>\
      </class></classes></package></packages></coverage>".into()
}
fn expected()->cov::ExpectedRun {
    cov::ExpectedRun{root_identity:"root-1".into(),epoch:EPOCH.into(),attempt_id:42,
        target:"unit".into(),build_identity:"build-v1".into(),binary_identity:"binary-v1".into(),
        collector:"coverlet".into(),collector_version:"collector-v1".into(),source_identity:"sources-1".into(),
        started_ms:9_000,now_ms:12_000,outcome:cov::RunOutcome::Failed,
        aliases:vec![cov::Alias{collector_path:"src/A.cs".into(),target:"unit".into(),
            source_path:"src/A.cs".into(),source_sha256:A.into()}]}
}
fn receipt_for(xml:&[u8],e:&cov::ExpectedRun)->cov::CoverageReceipt {
    cov::CoverageReceipt{root_identity:e.root_identity.clone(),epoch:e.epoch.clone(),attempt_id:e.attempt_id,
        target:e.target.clone(),build_identity:e.build_identity.clone(),binary_identity:e.binary_identity.clone(),
        collector:e.collector.clone(),collector_version:e.collector_version.clone(),source_identity:e.source_identity.clone(),
        collected_ms:10_000,artifact_sha256:sha256_hex(xml),aliases:e.aliases.clone()}
}
#[test]
fn failed_run_lines_and_aggregate_branches_match_csharp_importer() {
    let xml=xml(); let expected=expected();let receipt=receipt_for(xml.as_bytes(),&expected);
    let result=cov::inspect_unattested(xml.as_bytes(),&receipt,&expected);
    assert_eq!(cov::ImportState::Imported,result.state);
    assert_eq!(cov::LineState::Uncovered,result.line_state("unit","src/A.cs",12,A));
    assert_eq!(cov::LineState::Unknown,result.line_state("unit","src/A.cs",12,B));
    assert_eq!(cov::LineState::Unknown,result.line_state("unit","src/A.cs",13,A));
    assert_eq!(0,result.lines[0].hits); // class-level, not method duplicate
    assert_eq!((1,2),(result.branch_aggregates[0].covered,result.branch_aggregates[0].valid));
    assert_eq!(cov::RunOutcome::Failed,result.totals.unwrap().outcome);
}
#[test]
fn stale_identity_wrong_digest_and_alias_changes_are_mismatched() {
    let xml=xml();let e=expected();let r=receipt_for(xml.as_bytes(),&e);
    let mut wrong=r.clone();wrong.binary_identity="binary-other".into();
    assert_eq!(cov::ImportState::Mismatched,cov::inspect_unattested(xml.as_bytes(),&wrong,&e).state);
    assert_eq!(cov::ImportState::Mismatched,cov::inspect_unattested(xml.replace("hits='0'","hits='1'").as_bytes(),&r,&e).state);
    let mut wrong=r.clone();wrong.aliases[0].source_sha256=B.into();
    assert_eq!(cov::ImportState::Mismatched,cov::inspect_unattested(xml.as_bytes(),&wrong,&e).state);
    let mut wrong=r;wrong.collected_ms=8_000;
    assert_eq!(cov::ImportState::Expired,cov::inspect_unattested(xml.as_bytes(),&wrong,&e).state);
}
#[test]
fn corrupted_dtd_depth_and_duplicate_class_counters_are_rejected() {
    for s in ["", "<coverage", "<!DOCTYPE coverage [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><coverage/>",
        "<coverage lines-covered='-1' lines-valid='1' branches-covered='0' branches-valid='0'><packages/></coverage>"] {
        let e=expected();let r=receipt_for(s.as_bytes(),&e);
        assert_eq!(cov::ImportState::Malformed,cov::inspect_unattested(s.as_bytes(),&r,&e).state);
    }
    let duplicate=xml().replace("</class>","<lines><line number='12' hits='1'/></lines></class>");
    let e=expected();let r=receipt_for(duplicate.as_bytes(),&e);
    assert_eq!(cov::ImportState::Malformed,cov::inspect_unattested(duplicate.as_bytes(),&r,&e).state);
    for condition in ["100% (2/1)","???","200% (2/2)","25% (x/2)"] {
        let s=xml().replace("50% (1/2)",condition);let e=expected();let r=receipt_for(s.as_bytes(),&e);
        assert_eq!(cov::ImportState::Malformed,cov::inspect_unattested(s.as_bytes(),&r,&e).state);
    }
}
#[test]
fn unmapped_unknown_and_noncompleted_run_never_mint_hit() {
    let xml=xml().replace("src/A.cs","another/A.cs");let e=expected();let r=receipt_for(xml.as_bytes(),&e);
    let result=cov::inspect_unattested(xml.as_bytes(),&r,&e);
    assert_eq!(cov::ImportState::UnmappedScope,result.state);
    assert!(result.lines.is_empty());assert_eq!("unmapped_alias",result.unknown_scope[0].reason);
    let e={let mut e=expected();e.outcome=cov::RunOutcome::Skipped;e};
    let r=receipt_for(xml.as_bytes(),&e);
    assert_eq!(cov::ImportState::Missing,cov::inspect_unattested(xml.as_bytes(),&r,&e).state);
    let e={let mut e=expected();e.outcome=cov::RunOutcome::ZeroDiscovered;e};
    let r=receipt_for(xml.as_bytes(),&e);
    assert_eq!(cov::ImportState::Missing,cov::inspect_unattested(xml.as_bytes(),&r,&e).state);
    let e={let mut e=expected();e.outcome=cov::RunOutcome::Incomplete;e};
    let r=receipt_for(xml.as_bytes(),&e);
    assert_eq!(cov::ImportState::Missing,cov::inspect_unattested(xml.as_bytes(),&r,&e).state);
}

#[cfg(target_os="linux")]
mod linux_capture {
    use super::*;
    use std::{fs,os::unix::fs::symlink,path::PathBuf,time::{SystemTime,UNIX_EPOCH}};
    fn owned_file()->(PathBuf,PathBuf) {
        let unique=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root=std::env::temp_dir().join(format!("sg-mig-13-{}-{unique}",std::process::id()));
        fs::create_dir(&root).unwrap();
        let file=root.join("report.xml");fs::write(&file,xml()).unwrap();
        (root,file)
    }
    fn facts()->capture::ReadScopeFacts {
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        capture::ReadScopeFacts{root_identity:"root-1".into(),epoch:EPOCH.into(),attempt_id:42,
            target:"unit".into(),build_identity:"build-v1".into(),owned_and_drained:true,
            mapping_trusted:true,runtime_closure_complete:true,
            started_ms:now-60_000,finished_ms:now+2_000,
            lease_created_ms:now-59_000,now_ms:now,
            authorization_reference:"grant-1".into()}
    }
    #[test]
    fn physical_snapshot_binds_failed_run_and_verifier_cannot_claim_exact_arm() {
        let (root,_file)=owned_file();
        let facts=facts();
        let snapshot=capture::capture_coverage(&root,"report.xml",&facts).unwrap();
        let mut expected=expected();expected.started_ms=facts.started_ms;expected.now_ms=facts.now_ms;
        let mut receipt=receipt_for(snapshot.bytes(),&expected);receipt.collected_ms=facts.now_ms;
        let accepted=cov::import_accepted(&snapshot,&receipt,&expected).unwrap();
        let mapping=slophawk_receipts_coverage::verification::LineAttribution {
            id:"line-12".into(),detector_id:"coverage".into(),source_path:"src/A.cs".into(),line:12,
            mapping:slophawk_receipts_coverage::verification::MappingStrength::ExactDocumentLine,
            scope:slophawk_receipts_coverage::verification::ScopeFields {
                context_id:"ctx".into(),target_id:"unit".into(),symbol_id:"A.M".into(),
                behavior_id:"branch-12-arm-0".into(),source_digest:A.into(),build_id:"build-v1".into()}
        };
        use slophawk_receipts_coverage::verification as bridge;
        let projection=bridge::project(&accepted,&[mapping.clone()]);
        assert_eq!(1,projection.evidence.len());
        assert_eq!(slophawk_verification::VerificationCounterMapping::LineOnly,
            projection.evidence[0].fact.counter_mapping);
        assert_eq!(slophawk_verification::VerificationRunOutcome::Failed,
            projection.evidence[0].for_evaluation().run_outcome);
        assert!(!bridge::branch_arm_is_observed_from_cobertura(&accepted,&mapping));
        assert_eq!(cov::LineState::Uncovered,bridge::line_state_for_mapping(&accepted,&mapping));
        let mut stale=mapping.clone();stale.scope.build_id="stale-build".into();
        assert_eq!(cov::LineState::Unknown,bridge::line_state_for_mapping(&accepted,&stale));
        let mut possible=mapping.clone();possible.mapping=bridge::MappingStrength::Possible;
        assert_eq!(cov::LineState::Unknown,bridge::line_state_for_mapping(&accepted,&possible));
        let scope=mapping.scope.as_verification_scope();
        let request=slophawk_verification::typed::TypedVerificationRequest {
            policy:slophawk_verification::VerificationPolicy {version:"v1".into(),
                default_mode:slophawk_verification::VerificationPolicyMode::Advisory,rules:vec![]},
            changes:vec![slophawk_verification::typed::TypedVerificationChange {
                kind:slophawk_verification::typed::VerificationChangeKind::EditedBranch,
                change:slophawk_verification::VerificationChange {id:"change".into(),scope,
                    changed_range:"12:1".into(),reasons:vec!["edited".into()]} }],
            searches:vec![],evidence:vec![],
        };
        let batch=bridge::evaluate_with_coverage(request,&projection,None).unwrap();
        assert_eq!(slophawk_verification::VerificationState::Unknown,batch.decisions[0].state);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn links_hardlinks_and_unattested_scopes_fail_closed() {
        let (root,file)=owned_file();
        let mut invalid=facts();invalid.owned_and_drained=false;
        assert!(capture::capture_coverage(&root,"report.xml",&invalid).is_err());
        let linked=root.join("linked.xml");symlink(&file,&linked).unwrap();
        assert!(capture::capture_coverage(&root,"linked.xml",&facts()).is_err());
        let hard=root.join("hard.xml");fs::hard_link(&file,&hard).unwrap();
        assert!(capture::capture_coverage(&root,"report.xml",&facts()).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn json_unknown_fields_are_not_silently_accepted_as_schema_v1() {
    let mut input=serde_json::to_value(inputs()).unwrap();
    input.as_object_mut().unwrap().insert("unrecognized_schema".into(),serde_json::json!(2));
    assert!(serde_json::from_value::<build::BuildInputs>(input).is_err());
    let mut receipt=serde_json::to_value(details()).unwrap();
    receipt.as_object_mut().unwrap().insert("untrusted_override".into(),serde_json::json!(true));
    assert!(serde_json::from_value::<test::TestReceipt>(receipt).is_err());
    let mut coverage=serde_json::to_value(expected()).unwrap();
    coverage.as_object_mut().unwrap().insert("schema".into(),serde_json::json!("unknown"));
    assert!(serde_json::from_value::<cov::ExpectedRun>(coverage).is_err());
}

#[test]
fn missing_counters_and_ambiguous_alias_never_mean_uncovered() {
    let missing=xml().replace("<line number='12' hits='0' branch='true' condition-coverage='50% (1/2)'/>","");
    let e=expected(); let r=receipt_for(missing.as_bytes(),&e);
    let result=cov::inspect_unattested(missing.as_bytes(),&r,&e);
    assert_eq!(cov::ImportState::Imported,result.state);
    assert_eq!(cov::LineState::Unknown,result.line_state("unit","src/A.cs",12,A));
    let mut e=expected(); e.aliases.push(cov::Alias {
        collector_path:"src/A.cs".into(),target:"unit".into(),
        source_path:"src/B.cs".into(),source_sha256:B.into(),
    });
    let xml=xml(); let r=receipt_for(xml.as_bytes(),&e);
    let result=cov::inspect_unattested(xml.as_bytes(),&r,&e);
    assert_eq!(cov::ImportState::UnmappedScope,result.state);
    assert_eq!("ambiguous_alias",result.unknown_scope[0].reason);
    assert_eq!(cov::LineState::Unknown,result.line_state("unit","src/A.cs",12,A));
}

#[test]
fn size_limits_and_unsupported_xml_are_rejected_without_receipts() {
    let oversized=vec![b'x';slophawk_receipts_coverage::MAX_COVERAGE_BYTES+1];
    let e=expected();let r=receipt_for(&oversized,&e);
    assert_eq!(cov::ImportState::Malformed,cov::inspect_unattested(&oversized,&r,&e).state);
    let bogus=b"<example />";let r=receipt_for(bogus,&e);
    assert_eq!(cov::ImportState::Malformed,cov::inspect_unattested(bogus,&r,&e).state);
    let alternate=b"<coverage xmlns='urn:unsupported' lines-covered='0' lines-valid='0' branches-covered='0' branches-valid='0'><packages/></coverage>";
    let r=receipt_for(alternate,&e);
    assert_eq!(cov::ImportState::Malformed,cov::inspect_unattested(alternate,&r,&e).state);
}

#[cfg(not(target_os="linux"))]
#[test]
fn unsupported_physical_platform_does_not_claim_artifact_authority() {
    let facts=capture::ReadScopeFacts {root_identity:"root-1".into(),epoch:EPOCH.into(),attempt_id:42,
        target:"unit".into(),build_identity:"build-v1".into(),owned_and_drained:true,
        mapping_trusted:true,runtime_closure_complete:true,
        started_ms:1_000,finished_ms:2_000,lease_created_ms:1_000,now_ms:2_000,
        authorization_reference:"grant-1".into()};
    let root = std::env::temp_dir();
    let result=capture::capture_coverage(&root,"report.xml",&facts);
    assert_eq!(slophawk_receipts_coverage::Status::Unsupported,result.unwrap_err().status);
}
