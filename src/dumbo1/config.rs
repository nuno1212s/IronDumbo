use crate::quorum_info::quorum_info::ThresholdKeys;
use getset::Getters;

#[derive(Debug, Clone, Getters)]
pub struct Dumbo1Config {
    // Keys for this specific node instance
    #[get = "pub"]
    pub threshold_keys: ThresholdKeys,
}
