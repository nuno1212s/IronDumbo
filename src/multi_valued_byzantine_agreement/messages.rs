use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::CombinedSignature;
use atlas_common::node_id::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum MVBAMessage {
    /// Gossips a validated PRBC entry to peers that may not have it yet, so
    /// they have something to vote on for `owner`'s ABA slot. Purely
    /// informational: it does not itself feed into any ABA vote (votes are
    /// locked in once `propose` is called).
    Entry {
        owner: NodeId,
        digest: Digest,
        signature: CombinedSignature,
    },
    /// A sub-protocol message for the ABA instance deciding whether
    /// `owner`'s entry is included in the agreed set.
    Vote {
        owner: NodeId,
        message: AsyncBinaryAgreementMessage,
    },
}
