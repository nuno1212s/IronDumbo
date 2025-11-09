use crate::quorum_info::quorum_info::QuorumInfo;
use atlas_common::crypto::hash::{Context, Digest};
use atlas_communication::message::StoredMessage;
use atlas_core::request_pre_processing::BatchOutput;
use std::sync::Mutex;

#[derive()]
struct RequestAggregator<RQ> {
    batch_output: BatchOutput<RQ>,
    quorum_info: QuorumInfo,
    current_batch: Mutex<Option<CurrentBatch<RQ>>>
}

struct CurrentBatch<RQ>(Vec<StoredMessage<RQ>>, Digest);

impl<RQ> RequestAggregator<RQ> {
    pub fn new(batch_output: BatchOutput<RQ>, quorum_info: QuorumInfo) -> Self {
        Self {
            batch_output,
            quorum_info,
            current_batch: Mutex::new(None),
        }
    }

    pub fn collect_requests(&self) -> Vec<StoredMessage<RQ>> {
        let mut request = Vec::new();

        while let Ok(batch) = self.batch_output.try_recv() {
            request.append(&mut batch.into())
        }

        request
    }

    fn run(&self) {
        let mut requests = self.collect_requests();

        let mut batch_guard = self.current_batch.lock().unwrap();

        match &mut *batch_guard {
            None => {
                let digest = Self::calculate_digest_for(&requests);

                *batch_guard = Some(CurrentBatch(requests, digest))
            }
            Some(batch) => {
                batch.0.append(&mut requests);

                batch.1 = Self::calculate_digest_for(&batch.0);
            }
        }
    }

    fn calculate_digest_for(requests: &[StoredMessage<RQ>]) -> Digest {
        let mut context = Context::new();

        requests.iter()
            .for_each(|request| context.update(request.header().digest().as_ref()));

        context.finish()
    }
}

