use atlas_common::crypto::threshold_crypto::PartialSignature;
use getset::{CopyGetters, Getters};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Getters, CopyGetters, Serialize, Deserialize)]
pub(super) struct AsyncBinaryAgreementMessage {
    #[get_copy = "pub(super)"]
    round: usize,
    #[get = "pub"]
    message_type: AsyncBinaryAgreementMessageType,
}

impl AsyncBinaryAgreementMessage {
    pub(super) fn new(message_type: AsyncBinaryAgreementMessageType, round: usize) -> Self {
        Self {
            message_type,
            round,
        }
    }

    pub(super) fn into_inner(self) -> (usize, AsyncBinaryAgreementMessageType) {
        (self.round, self.message_type)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum AsyncBinaryAgreementMessageType {
    Val {
        estimate: bool,
    },
    Aux {
        accepted_estimates: Vec<bool>,
    },
    Conf {
        feasible_values: Vec<bool>,
        partial_signature: PartialSignature,
    },
    Finish {
        value: bool,
    },
}
