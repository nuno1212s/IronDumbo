use atlas_common::ordering::{Orderable, SeqNo};
use atlas_common::serialization_helper::SerMsg;
use atlas_communication::message::Header;
use atlas_communication::reconfiguration::NetworkInformationProvider;
use atlas_core::ordering_protocol::networking::serialize::{
    OrderProtocolVerificationHelper, OrderingProtocolMessage,
};
use getset::Getters;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use std::sync::Arc;
use atlas_common::node_id::NodeId;

/// A message used in the Dumbo1 protocol.
/// This struct encapsulates a message round and the message type.
/// See [`DumboMessageType`]
#[derive(Clone, Serialize, Deserialize, Getters)]
pub struct DumboMessage<RBM, IRBM, AM, CEM> {
    message_round: SeqNo,
    #[get = "pub"]
    message_type: DumboMessageType<RBM, IRBM, AM, CEM>,
}

impl<RBM, IRBM, AM, CEM> DumboMessage<RBM, IRBM, AM, CEM> {
    pub fn new(message_round: SeqNo, message_type: DumboMessageType<RBM, IRBM, AM, CEM>) -> Self {
        Self {
            message_round,
            message_type,
        }
    }
}

impl<RBM, IRBM, AM, CEM> Orderable for DumboMessage<RBM, IRBM, AM, CEM> {
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
pub enum DumboMessageType<RBM, IRBM, AM, CEM> {
    // The node id of the "owner" of the reliable broadcast execution
    ReliableBroadcast(NodeId, RBM),
    IndexReliableBroadcast(NodeId, IRBM),
    AsyncBinaryAgreement(NodeId, AM),
    CommitteeElectionMessage(CEM),
}

pub struct DumboSerialization<RQ, RBM, IRBM, AM, CEM>(PhantomData<fn(RQ, RBM, IRBM, AM, CEM)>);

impl<RQ, RBM, IRBM, AM, CEM> OrderingProtocolMessage<RQ>
    for DumboSerialization<RQ, RBM, IRBM, AM, CEM>
where
    RQ: 'static,
    RBM: SerMsg,
    IRBM: SerMsg,
    AM: SerMsg,
    CEM: SerMsg,
{
    type ProtocolMessage = DumboMessage<RBM, IRBM, AM, CEM>;
    type DecisionMetadata = ();
    type DecisionAdditionalInfo = ();

    fn internally_verify_message<NI, OPVH>(
        _network_info: &Arc<NI>,
        _header: &Header,
        _message: &Self::ProtocolMessage,
    ) -> atlas_common::error::Result<()>
    where
        NI: NetworkInformationProvider,
        OPVH: OrderProtocolVerificationHelper<RQ, Self, NI>,
        Self: Sized,
    {
        todo!()
    }
}
