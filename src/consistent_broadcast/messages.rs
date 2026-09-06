use atlas_common::crypto::threshold_crypto::{CombinedSignature, PartialSignature};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum CBCMessage<V> {
    /// `(SEND, v)` -- Algorithm 6 line 2. Sent by the owner only, once.
    Send(V),
    /// `(ECHO, sigma_i)` -- line 11. Unicast to the owner only (not
    /// broadcast): only the owner ever collects echoes, matching the
    /// algorithm's "send ... to P_s".
    Echo(PartialSignature),
    /// `(Finish, v, sigma)` -- line 8. Multicast by the owner once it holds
    /// `2f+1` valid echoes. Carries the value itself (not just its digest)
    /// so it is independently, self-certifyingly verifiable by any node --
    /// including one that never saw SEND.
    Finish(V, CombinedSignature),
}

impl<V> PartialEq for CBCMessage<V>
where
    V: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CBCMessage::Send(a), CBCMessage::Send(b)) => a == b,
            (CBCMessage::Echo(a), CBCMessage::Echo(b)) => a == b,
            (CBCMessage::Finish(av, asig), CBCMessage::Finish(bv, bsig)) => {
                av == bv && asig == bsig
            }
            _ => false,
        }
    }
}

impl<V> Eq for CBCMessage<V> where V: Eq {}
