use crate::quorum_info::quorum_info::ThresholdKeys;
use getset::Getters;

#[derive(Debug, Clone, Getters)]
pub struct Dumbo2Config {
    #[get = "pub"]
    pub threshold_keys: ThresholdKeys,
}
