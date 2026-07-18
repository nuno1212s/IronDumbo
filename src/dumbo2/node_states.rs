use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::CombinedSignature;
use std::fmt::Debug;

/// Our view of a single quorum member's PRBC broadcast within a Dumbo2
/// round: either still running, or finished with its value, digest, and
/// combined-signature proof of delivery.
pub(super) enum NodeState2<RQ, PR> {
    RunningPRBC(PR),
    Done {
        value: RQ,
        digest: Digest,
        signature: CombinedSignature,
    },
}

impl<RQ, PR> NodeState2<RQ, PR> {
    pub(super) fn is_done(&self) -> bool {
        matches!(self, NodeState2::Done { .. })
    }
}

impl<RQ, PR> Debug for NodeState2<RQ, PR>
where
    PR: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeState2::RunningPRBC(prbc) => write!(f, "RunningPRBC({prbc:?})"),
            NodeState2::Done { digest, .. } => write!(f, "Done(digest: {digest:?})"),
        }
    }
}
