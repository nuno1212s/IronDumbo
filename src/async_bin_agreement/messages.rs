use atlas_common::crypto::threshold_crypto::PartialSignature;
use getset::{CopyGetters, Getters};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Getters, CopyGetters, Serialize, Deserialize)]
pub(crate) struct AsyncBinaryAgreementMessage {
    #[get_copy = "pub(crate)"]
    round: usize,
    #[get = "pub"]
    message_type: AsyncBinaryAgreementMessageType,
}

impl AsyncBinaryAgreementMessage {
    pub(crate) fn new(message_type: AsyncBinaryAgreementMessageType, round: usize) -> Self {
        Self {
            message_type,
            round,
        }
    }

    pub(crate) fn into_inner(self) -> (usize, AsyncBinaryAgreementMessageType) {
        (self.round, self.message_type)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AsyncBinaryAgreementMessageType {
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
