use atlas_common::node_id::NodeId;
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

/// A message used in the Dumbo2 protocol: an epoch/sequence number plus the
/// payload. See [`Dumbo2MessageType`].
#[derive(Clone, Serialize, Deserialize, Getters)]
pub struct Dumbo2Message<PM, MM> {
    message_round: SeqNo,
    #[get = "pub"]
    message_type: Dumbo2MessageType<PM, MM>,
}

impl<PM, MM> Dumbo2Message<PM, MM> {
    pub fn new(message_round: SeqNo, message_type: Dumbo2MessageType<PM, MM>) -> Self {
        Self {
            message_round,
            message_type,
        }
    }
}

impl<PM, MM> Orderable for Dumbo2Message<PM, MM> {
    fn sequence_number(&self) -> SeqNo {
        self.message_round
    }
}

/// Messages used in the Dumbo2 protocol: PRBC (tagged with which node's
/// broadcast it belongs to) and MVBA. Unlike Dumbo1, there is no
/// IndexReliableBroadcast or CommitteeElection phase.
#[derive(Clone, Serialize, Deserialize)]
pub enum Dumbo2MessageType<PM, MM> {
    PRBC(NodeId, PM),
    MVBA(MM),
}

pub struct Dumbo2Serialization<RQ, PM, MM>(PhantomData<fn(RQ, PM, MM)>);

impl<RQ, PM, MM> OrderingProtocolMessage<RQ> for Dumbo2Serialization<RQ, PM, MM>
where
    RQ: 'static,
    PM: SerMsg,
    MM: SerMsg,
{
    type ProtocolMessage = Dumbo2Message<PM, MM>;
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
        // As in Dumbo1, message authenticity is established by the signed
        // envelope; there are no further semantic invariants to check here.
        Ok(())
    }
}
