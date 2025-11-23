use atlas_common::crypto::hash::Digest;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum ReliableBroadcastMessage<RQ> {
    Send(RQ),
    Echo(Digest),
    Ready(Digest),
}

impl<RQ> PartialEq for ReliableBroadcastMessage<RQ> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ReliableBroadcastMessage::Send(_),
                ReliableBroadcastMessage::Send(_),
            ) => false,
            (ReliableBroadcastMessage::Echo(d1), ReliableBroadcastMessage::Echo(d2)) => d1 == d2,
            (ReliableBroadcastMessage::Ready(d1), ReliableBroadcastMessage::Ready(d2)) => d1 == d2,
            _ => false,
        }
    }
}

impl<RQ> Eq for ReliableBroadcastMessage<RQ> where RQ: PartialEq {}

impl<RQ> Clone for ReliableBroadcastMessage<RQ>
where
    RQ: Clone,
{
    fn clone(&self) -> Self {
        match self {
            ReliableBroadcastMessage::Send(messages) => {
                ReliableBroadcastMessage::Send(messages.clone())
            }
            ReliableBroadcastMessage::Echo(digest) => ReliableBroadcastMessage::Echo(*digest),
            ReliableBroadcastMessage::Ready(digest) => ReliableBroadcastMessage::Ready(*digest),
        }
    }
}
