use crate::quorum_info::quorum_info::QuorumInfo;
use atlas_common::crypto::hash::{Context, Digest};
use atlas_communication::message::StoredMessage;
use atlas_core::request_pre_processing::BatchOutput;
use std::sync::Mutex;

struct CurrentBatch<RQ>(Vec<StoredMessage<RQ>>, Digest);

#[derive()]
pub(super) struct RequestAggregator<RQ> {
    batch_output: BatchOutput<RQ>,
    quorum_info: QuorumInfo,
    current_batch: Mutex<Option<CurrentBatch<RQ>>>,
}

impl<RQ> RequestAggregator<RQ> {
    pub fn new(batch_output: BatchOutput<RQ>, quorum_info: QuorumInfo) -> Self {
        Self {
            batch_output,
            quorum_info,
            current_batch: Mutex::new(None),
        }
    }

    pub fn get_batch_and_reset(&self) -> (Vec<StoredMessage<RQ>>, Digest) {
        let mut batch_guard = self.current_batch.lock().unwrap();

        let batch = batch_guard
            .take()
            .unwrap_or(CurrentBatch(Vec::new(), Digest::blank()));

        (batch.0, batch.1)
    }

    pub fn return_requests(&self, requests: Vec<StoredMessage<RQ>>) {
        self.add_messages_to_current_batch(requests);
    }

    fn collect_requests(&self) -> Vec<StoredMessage<RQ>> {
        let mut request = Vec::new();

        while let Ok(batch) = self.batch_output.try_recv() {
            request.append(&mut batch.into())
        }

        request
    }

    fn run(&self) {
        let requests = self.collect_requests();

        self.add_messages_to_current_batch(requests);
    }

    fn add_messages_to_current_batch(&self, mut requests: Vec<StoredMessage<RQ>>) {
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

        requests
            .iter()
            .for_each(|request| context.update(request.header().digest().as_ref()));

        context.finish()
    }
}
