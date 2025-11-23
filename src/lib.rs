// This is mostly for the types and restrictions to be clear
// I do not enable the feature since it has some breaking bugs which cause the code
// To not compile due to type alias bounds not being allowed in some cases
#![allow(type_alias_bounds)]
extern crate core;

mod reliable_broadcast {
    pub mod messages;
    pub mod reliable_broadcast;

    #[cfg(test)]
    pub mod test {
        pub mod reliable_broadcast_test;
    }
}

mod quorum_info {
    pub mod quorum_info;
}

mod async_bin_agreement {
    pub mod async_bin_agreement;
    pub mod async_bin_agreement_round;
    pub mod messages;
    pub mod pending_messages;
    #[cfg(test)]
    pub mod test {
        pub mod async_bin_agreement_test;
        pub mod message_handling_test;
    }
}

pub mod aba;
pub mod rbc;
mod rq_aggregator;
mod committee_election;
mod consensus_rqs;

pub mod dumbo1 {
    pub mod protocol;
    mod epoch;
    mod epoch_round_state;
    mod node_states;
    mod message;
    mod network;
    mod pending_messages;
    mod config;
}
