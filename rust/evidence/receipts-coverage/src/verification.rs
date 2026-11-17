//! Bridge to the **published** independent verification rule engine. This is
//! intentionally conservative: Cobertura line hits and condition percentages
//! never become exact branch-arm counters or per-test proof.
use crate::cobertura::{AcceptedCoverage,LineState,RunOutcome};
use serde::{Deserialize, Serialize};
use slophawk_verification::{
    VerificationScope,VerificationEvidence,VerificationEvidenceKind,VerificationBinding,
    VerificationCounterMapping,VerificationRunOutcome,VerificationBatch,VerificationLimits,
    VerificationInputError,EDITED_BRANCH_RULE_ID,
};
use slophawk_verification::typed::{ProviderRunDisposition,ProviderVerificationEvidence,
    TypedVerificationRequest,evaluate_typed};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum MappingStrength { Possible, ExactDocumentLine }

/// A source-map attribution is external to the collector and must identify the
/// same current source digest, build and concrete target context as the verifier.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LineAttribution {
    pub id:String, pub detector_id:String, pub source_path:String, pub line:i32,
    pub mapping:MappingStrength, pub scope:ScopeFields,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScopeFields {
    pub context_id:String, pub target_id:String, pub symbol_id:String, pub behavior_id:String,
    pub source_digest:String, pub build_id:String,
}
impl ScopeFields {
    pub fn as_verification_scope(&self)->VerificationScope {
        VerificationScope {context_id:self.context_id.clone(),target_id:self.target_id.clone(),
            symbol_id:self.symbol_id.clone(),behavior_id:self.behavior_id.clone(),
            source_digest:self.source_digest.clone(),build_id:self.build_id.clone()}
    }
}
#[derive(Clone, Debug)]
pub struct Projection {
    pub evidence:Vec<ProviderVerificationEvidence>,
    pub excluded:Vec<(String,String)>,
}

pub fn project(accepted:&AcceptedCoverage, mappings:&[LineAttribution])->Projection {
    let result=accepted.result();
    let mut projection=Projection{evidence:vec![],excluded:vec![]};
    let Some(totals)=&result.totals else {return projection;};
    let disposition=match totals.outcome {
        RunOutcome::Passed=>ProviderRunDisposition::Passed,
        RunOutcome::Failed=>ProviderRunDisposition::Failed,
        RunOutcome::Skipped=>ProviderRunDisposition::Skipped,
        RunOutcome::ZeroDiscovered=>ProviderRunDisposition::ZeroDiscovered,
        RunOutcome::Unadmitted=>ProviderRunDisposition::Unadmitted,
        RunOutcome::Incomplete=>ProviderRunDisposition::Incomplete,
    };
    // No mapping can bypass the current-source/build test. A duplicate mapping
    // to one change would amplify the same aggregate, so reject duplicates.
    let mut used=std::collections::HashSet::new();
    for m in mappings {
        let key=(&m.scope.context_id,&m.scope.target_id,&m.scope.symbol_id,
            &m.scope.behavior_id,&m.source_path,m.line);
        if !used.insert(key) {projection.excluded.push((m.id.clone(),"duplicate_mapping".into()));continue;}
        if m.scope.target_id!=totals.target || m.scope.build_id!=totals.build_identity ||
            !crate::valid_digest(&m.scope.source_digest) || m.line<=0 {
            projection.excluded.push((m.id.clone(),"target_or_build_stale".into()));continue;
        }
        let line=result.lines.iter().find(|l|l.target==m.scope.target_id && l.source_path==m.source_path &&
            l.number==m.line && crate::digest_matches(&l.source_sha256,&m.scope.source_digest));
        let Some(line)=line else {projection.excluded.push((m.id.clone(),"unmapped_or_stale_source".into()));continue;};
        let exact=m.mapping==MappingStrength::ExactDocumentLine;
        let binding=if exact {VerificationBinding::Exact} else {VerificationBinding::Possible};
        // This is an honest line counter, never an exact branch-arm measurement.
        let fact=VerificationEvidence {id:m.id.clone(),rule_id:EDITED_BRANCH_RULE_ID.into(),
            detector_id:m.detector_id.clone(),scope:m.scope.as_verification_scope(),
            kind:VerificationEvidenceKind::BranchCounter,binding,valid_receipt:true,
            exact_target_mapping:exact,counter_mapping:VerificationCounterMapping::LineOnly,
            hit_count:Some(line.hits),run_outcome:match totals.outcome {
                RunOutcome::Passed=>VerificationRunOutcome::Passed,
                RunOutcome::Failed=>VerificationRunOutcome::Failed,
                RunOutcome::Skipped=>VerificationRunOutcome::Skipped,
                RunOutcome::Unadmitted=>VerificationRunOutcome::NotRun,
                _=>VerificationRunOutcome::Incomplete,
            },executed_behavior:false,relevant_assertion:false};
        projection.evidence.push(ProviderVerificationEvidence{fact,disposition});
    }
    projection
}

/// Append the accepted facts and evaluate through the published mixed-rule
/// evaluator; callers supply independent change/search/policy inputs.
pub fn evaluate_with_coverage(mut request:TypedVerificationRequest,projection:&Projection,
    limits:Option<&VerificationLimits>)->Result<VerificationBatch,VerificationInputError> {
    request.evidence.extend(projection.evidence.iter().map(|e|e.for_evaluation()));
    evaluate_typed(&request,limits)
}

/// No caller may claim an exact branch from an imported aggregate.
pub fn branch_arm_is_observed_from_cobertura(_result:&AcceptedCoverage,
    _mapping:&LineAttribution)->bool {false}

pub fn line_state_for_mapping(accepted:&AcceptedCoverage,mapping:&LineAttribution)->LineState {
    let Some(totals)=&accepted.result().totals else {return LineState::Unknown;};
    if mapping.mapping!=MappingStrength::ExactDocumentLine ||
        mapping.scope.target_id!=totals.target || mapping.scope.build_id!=totals.build_identity ||
        !crate::valid_digest(&mapping.scope.source_digest) {return LineState::Unknown;}
    accepted.result().line_state(&mapping.scope.target_id,&mapping.source_path,
        mapping.line,&mapping.scope.source_digest)
}
