use atlas_common::crypto::hash::Digest;
use atlas_communication::message::StoredMessage;

#[derive(Debug)]
pub(crate) enum ReliableBroadcastMessage<RQ> {
    Send(Vec<StoredMessage<RQ>>, Digest),
    Echo(Digest),
    Ready(Digest),
}

impl<RQ> PartialEq for ReliableBroadcastMessage<RQ> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ReliableBroadcastMessage::Send(_, digest),
                ReliableBroadcastMessage::Send(_, digest2),
            ) => digest == digest2,
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
            ReliableBroadcastMessage::Send(messages, digest) => {
                ReliableBroadcastMessage::Send(messages.clone(), *digest)
            }
            ReliableBroadcastMessage::Echo(digest) => ReliableBroadcastMessage::Echo(*digest),
            ReliableBroadcastMessage::Ready(digest) => ReliableBroadcastMessage::Ready(*digest),
        }
    }
}
