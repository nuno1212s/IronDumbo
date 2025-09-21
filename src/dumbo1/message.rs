use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::Header;
use atlas_communication::reconfiguration::NetworkInformationProvider;
use atlas_core::ordering_protocol::networking::serialize::{
    OrderProtocolVerificationHelper, OrderingProtocolMessage,
};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use std::sync::Arc;
use getset::Getters;

/// A message used in the Dumbo1 protocol.
/// This struct encapsulates a message round and the message type.
/// See [`DumboMessageType`]
#[derive(Clone, Serialize, Deserialize, Getters)]
pub struct DumboMessage<RBM, AM, CEM>
{
    message_round: SeqNo,
    #[get = "pub"]
    message_type: DumboMessageType<RBM, AM, CEM>,
}

impl<RBM, AM, CEM> DumboMessage<RBM, AM, CEM>
{
    pub fn new(message_round: SeqNo, message_type: DumboMessageType<RBM, AM, CEM>) -> Self {
        Self {
            message_round,
            message_type,
        }
    }
}

impl<RBM, AM, CEM> Orderable for DumboMessage<RBM, AM, CEM>
{
    fn sequence_number(&self) -> SeqNo {
        self.message_round
    }
}

/// Messages used in the Dumbo1 protocol.
///
/// This enum encapsulates messages from the Reliable Broadcast,
/// Asynchronous Binary Agreement, and Committee Election protocols.
///
/// Each variant holds the corresponding message type.
#[derive(Clone, Serialize, Deserialize)]
pub enum DumboMessageType<RBM, AM, CEM>
{
    ReliableBroadcast(RBM),
    AsyncBinaryAgreement(AM),
    CommitteeElectionMessage(CEM),
}

pub struct DumboSerialization<RQ, RBM, AM, CEM>(PhantomData<fn(RQ, RBM, AM, CEM)>);

impl<RQ, RBM, AM, CEM> OrderingProtocolMessage<RQ> for DumboSerialization<RQ, RBM, AM, CEM>
where
    RQ: 'static,
    RBM: SerMsg,
    AM: SerMsg,
    CEM: SerMsg,
{
    type ProtocolMessage = DumboMessage<RBM, AM, CEM>;
    type DecisionMetadata = ();
    type DecisionAdditionalInfo = ();

    fn internally_verify_message<NI, OPVH>(
        network_info: &Arc<NI>,
        header: &Header,
        message: &Self::ProtocolMessage,
    ) -> atlas_common::error::Result<()>
    where
        NI: NetworkInformationProvider,
        OPVH: OrderProtocolVerificationHelper<RQ, Self, NI>,
        Self: Sized,
    {
        todo!()
    }
}
