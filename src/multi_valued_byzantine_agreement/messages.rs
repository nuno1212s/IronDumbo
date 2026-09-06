use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use crate::consistent_broadcast::messages::CBCMessage;
use crate::mvba::MVBAProposal;
use atlas_common::crypto::threshold_crypto::{CombinedSignature, PartialSignature};
use atlas_common::node_id::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum MVBAMessage {
    /// Routes to the per-owner CBC instance broadcasting that owner's
    /// candidate `MVBAProposal` ("echo the proposal", Fig. 4 step 1). One
    /// instance per quorum member, started eagerly.
    Cbc {
        owner: NodeId,
        message: CBCMessage<MVBAProposal>,
    },

    /// The "early commitment" vector (Cachin-Kursawe-Shoup's construction):
    /// `committed[i]` (indexed into a canonically sorted `quorum_members()`)
    /// is `true` iff this node had already CBC-delivered+`Q_r`-validated
    /// that owner's proposal at the moment this message was sent. Plain
    /// broadcast, no threshold crypto: equivocating it only affects the
    /// *speed* bound (expected O(1) rounds), never Agreement/Validity/
    /// Integrity.
    Commit { committed: Vec<bool> },

    /// This node's threshold-coin share for deriving the shared
    /// permutation (Algorithm 4's CShare, via `threshold_coin_tossing`).
    CoinShare { share: PartialSignature },

    /// A vote in the agreement loop for whichever candidate is currently
    /// active. `vote=true` must carry `proof`: the candidate's full
    /// `MVBAProposal` plus CBC's `2f+1`-threshold combined signature over
    /// it, so any recipient can adopt the value even if its own CBC
    /// instance for that candidate never finished.
    VVote {
        candidate: NodeId,
        vote: bool,
        proof: Option<(MVBAProposal, CombinedSignature)>,
    },

    /// A sub-protocol message for the ABA instance currently deciding
    /// whether `candidate`'s CBC'd proposal is the final decision.
    Aba {
        candidate: NodeId,
        message: AsyncBinaryAgreementMessage,
    },
}
