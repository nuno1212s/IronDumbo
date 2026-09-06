use crate::reliable_broadcast::messages::ErasureCodedPart;
use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{CombinedSignature, PartialSignature};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PRBCMessage {
    Val(ErasureCodedPart),
    Echo(ErasureCodedPart),
    Ready(Digest),
    /// A threshold-signature share over the finalized digest, sent once this
    /// node's inner reliable broadcast has finalized.
    Done(PartialSignature),
    /// The combined threshold signature, assembled from `f+1` `Done` shares.
    /// Self-certifying: any node can verify it against the quorum's public
    /// key without needing to collect its own shares.
    Finish(CombinedSignature),
}
