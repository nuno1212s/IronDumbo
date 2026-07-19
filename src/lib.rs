// This is mostly for the types and restrictions to be clear
// I do not enable the feature since it has some breaking bugs which cause the code
// To not compile due to type alias bounds not being allowed in some cases
#![allow(type_alias_bounds)]
extern crate core;

mod reliable_broadcast {
    pub mod erasure_coding;
    pub mod merkle;
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
        pub mod multi_node_test;
    }
}

pub mod aba;
mod cbc;
mod committee_election;
mod consensus_rqs;
pub mod mvba;
pub mod prbc;
pub mod rbc;
mod rq_aggregator;
mod threshold_coin_tossing;

mod consistent_broadcast {
    pub mod consistent_broadcast;
    pub mod messages;

    #[cfg(test)]
    pub mod test {
        pub mod cbc_test;
    }
}

mod provable_reliable_broadcast {
    pub mod messages;
    pub mod provable_reliable_broadcast;

    #[cfg(test)]
    pub mod test {
        pub mod prbc_test;
    }
}

mod multi_valued_byzantine_agreement {
    pub mod messages;
    pub mod mvba;

    #[cfg(test)]
    pub mod test {
        pub mod mvba_test;
    }
}

#[cfg(test)]
mod testing {
    pub mod assertions;
    pub mod byzantine;
    pub mod fixtures;
    pub mod network_sim;
}

pub mod dumbo1 {
    mod config;
    mod epoch;
    mod epoch_round_state;
    mod message;
    mod network;
    mod node_states;
    mod pending_messages;
    pub mod protocol;

    #[cfg(test)]
    pub mod test {
        pub mod dumbo1_integration_test;
        pub mod harness;
    }
}

pub mod dumbo2 {
    mod config;
    mod epoch;
    mod epoch_round_state;
    mod message;
    mod network;
    mod node_states;
    mod pending_messages;
    pub mod protocol;

    #[cfg(test)]
    pub mod test {
        pub mod dumbo2_integration_test;
        pub mod harness;
    }
}
