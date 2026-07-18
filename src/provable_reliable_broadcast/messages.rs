use atlas_common::crypto::hash::Digest;
use atlas_common::crypto::threshold_crypto::{CombinedSignature, PartialSignature};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum PRBCMessage<RQ> {
    Send(RQ),
    Echo(Digest),
    Ready(Digest),
    /// A threshold-signature share over the finalized digest, sent once this
    /// node's inner reliable broadcast has finalized.
    Done(PartialSignature),
    /// The combined threshold signature, assembled from `f+1` `Done` shares.
    /// Self-certifying: any node can verify it against the quorum's public
    /// key without needing to collect its own shares.
    Finish(CombinedSignature),
}

impl<RQ> Clone for PRBCMessage<RQ>
where
    RQ: Clone,
{
    fn clone(&self) -> Self {
        match self {
            PRBCMessage::Send(rq) => PRBCMessage::Send(rq.clone()),
            PRBCMessage::Echo(digest) => PRBCMessage::Echo(*digest),
            PRBCMessage::Ready(digest) => PRBCMessage::Ready(*digest),
            PRBCMessage::Done(sig) => PRBCMessage::Done(sig.clone()),
            PRBCMessage::Finish(sig) => PRBCMessage::Finish(sig.clone()),
        }
    }
}

impl<RQ> PartialEq for PRBCMessage<RQ> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PRBCMessage::Echo(a), PRBCMessage::Echo(b)) => a == b,
            (PRBCMessage::Ready(a), PRBCMessage::Ready(b)) => a == b,
            (PRBCMessage::Done(a), PRBCMessage::Done(b)) => a == b,
            (PRBCMessage::Finish(a), PRBCMessage::Finish(b)) => a == b,
            _ => false,
        }
    }
}

impl<RQ> Eq for PRBCMessage<RQ> where RQ: PartialEq {}
