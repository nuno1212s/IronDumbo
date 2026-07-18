use crate::aba::{ABAProtocol, AsyncBinaryAgreementResult, AsyncBinaryAgreementSendNode};
use crate::async_bin_agreement::async_bin_agreement::{ABAError, AsyncBinaryAgreement};
use crate::async_bin_agreement::messages::AsyncBinaryAgreementMessage;
use crate::multi_valued_byzantine_agreement::messages::MVBAMessage;
use crate::mvba::{MVBAProposal, MVBAProtocol, MVBAResult, MVBASendNode};
use crate::quorum_info::quorum_info::{QuorumInfo, ThresholdKeys};
use anyhow::anyhow;
use atlas_common::collections::HashMap;
use atlas_common::node_id::NodeId;
use atlas_communication::message::StoredMessage;
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};
use thiserror::Error;
use tracing::warn;

#[derive(Debug)]
enum AbaSlot {
    Running(AsyncBinaryAgreement),
    Done(bool),
}

/// Wraps an [`MVBASendNode`] so a per-owner [`AsyncBinaryAgreement`] instance
/// can send through it, tagging every outgoing message with which owner's
/// vote it belongs to (mirrors `provable_reliable_broadcast`'s
/// `RbcNetworkAdapter`).
struct AbaNetworkAdapter<'a, NT> {
    owner: NodeId,
    inner: &'a NT,
}

impl<'a, NT> AsyncBinaryAgreementSendNode<AsyncBinaryAgreementMessage> for AbaNetworkAdapter<'a, NT>
where
    NT: MVBASendNode<MVBAMessage>,
{
    fn broadcast_message<I>(
        &self,
        message: AsyncBinaryAgreementMessage,
        target: I,
    ) -> atlas_common::error::Result<()>
    where
        I: Iterator<Item = NodeId>,
    {
        let wrapped = MVBAMessage::Vote {
            owner: self.owner,
            message,
        };

        self.inner
            .broadcast(wrapped, target)
            .map_err(|failed| anyhow!("Failed to broadcast to some nodes: {:?}", failed))
    }
}

/// An MVBA instance built from `n` parallel ABA votes, one per quorum
/// member: "is this member's PRBC entry included in the agreed set?".
pub(crate) struct MultiValuedByzantineAgreement {
    quorum_info: QuorumInfo,
    threshold_keys: ThresholdKeys,
    abas: HashMap<NodeId, AbaSlot>,
    entries: HashMap<
        NodeId,
        (
            atlas_common::crypto::hash::Digest,
            atlas_common::crypto::threshold_crypto::CombinedSignature,
        ),
    >,
    proposed: bool,
    pending: VecDeque<StoredMessage<MVBAMessage>>,
}

impl MultiValuedByzantineAgreement {
    fn try_insert_entry<NT>(
        &mut self,
        owner: NodeId,
        digest: atlas_common::crypto::hash::Digest,
        signature: atlas_common::crypto::threshold_crypto::CombinedSignature,
        network: &NT,
    ) where
        NT: MVBASendNode<MVBAMessage>,
    {
        if self.entries.contains_key(&owner) {
            return;
        }

        if self
            .threshold_keys
            .public_key()
            .verify(digest.as_ref(), &signature)
            .is_err()
        {
            warn!("Rejecting invalid PRBC proof for {owner:?} in MVBA proposal");
            return;
        }

        self.entries.insert(owner, (digest, signature.clone()));

        let _ = network.broadcast(
            MVBAMessage::Entry {
                owner,
                digest,
                signature,
            },
            self.quorum_info.quorum_members().iter().cloned(),
        );
    }

    fn finalize_aba_slot(&mut self, owner: NodeId) -> Result<(), MVBAError> {
        let Some(AbaSlot::Running(_)) = self.abas.get(&owner) else {
            return Ok(());
        };

        let Some(AbaSlot::Running(aba)) = self.abas.remove(&owner) else {
            unreachable!("checked above");
        };

        let decision = aba.finalize().map_err(MVBAError::Aba)?;

        self.abas.insert(owner, AbaSlot::Done(decision));

        Ok(())
    }

    fn all_decided(&self) -> bool {
        self.abas.len() == self.quorum_info.quorum_members().len()
            && self
                .abas
                .values()
                .all(|slot| matches!(slot, AbaSlot::Done(_)))
    }
}

impl MVBAProtocol for MultiValuedByzantineAgreement {
    type Message = MVBAMessage;
    type Error = MVBAError;

    fn new(quorum_info: QuorumInfo, threshold_keys: ThresholdKeys) -> Self {
        Self {
            quorum_info,
            threshold_keys,
            abas: HashMap::default(),
            entries: HashMap::default(),
            proposed: false,
            pending: VecDeque::new(),
        }
    }

    fn propose<NT>(
        &mut self,
        proposal: MVBAProposal,
        network: &NT,
    ) -> Result<MVBAResult, Self::Error>
    where
        NT: MVBASendNode<Self::Message>,
    {
        for (owner, digest, signature) in proposal {
            self.try_insert_entry(owner, digest, signature, network);
        }

        let members = self.quorum_info.quorum_members().clone();

        for owner in members {
            let vote = self.entries.contains_key(&owner);
            let aba =
                AsyncBinaryAgreement::new(self.quorum_info.clone(), self.threshold_keys.clone());
            self.abas.insert(owner, AbaSlot::Running(aba));

            let adapter = AbaNetworkAdapter {
                owner,
                inner: network,
            };

            let result = {
                let Some(AbaSlot::Running(aba)) = self.abas.get_mut(&owner) else {
                    unreachable!("just inserted");
                };
                aba.provide_input_bit(vote, &adapter)
                    .map_err(MVBAError::Aba)?
            };

            if matches!(result, AsyncBinaryAgreementResult::Decided) {
                self.finalize_aba_slot(owner)?;
            }
        }

        self.proposed = true;

        let pending = std::mem::take(&mut self.pending);
        for message in pending {
            self.process_message(message, network)?;
        }

        if self.all_decided() {
            Ok(MVBAResult::Decided)
        } else {
            Ok(MVBAResult::Processed)
        }
    }

    fn process_message<NT>(
        &mut self,
        message: StoredMessage<Self::Message>,
        network: &NT,
    ) -> Result<MVBAResult, Self::Error>
    where
        NT: MVBASendNode<Self::Message>,
    {
        if !self.proposed {
            self.pending.push_back(message);
            return Ok(MVBAResult::MessageQueued);
        }

        let (header, msg) = message.into_inner();

        match msg {
            MVBAMessage::Entry {
                owner,
                digest,
                signature,
            } => {
                self.try_insert_entry(owner, digest, signature, network);
                Ok(MVBAResult::Processed)
            }
            MVBAMessage::Vote { owner, message } => {
                let Some(slot) = self.abas.get_mut(&owner) else {
                    return Ok(MVBAResult::MessageIgnored);
                };

                let AbaSlot::Running(aba) = slot else {
                    return Ok(MVBAResult::MessageIgnored);
                };

                let stored = StoredMessage::new(header, message);
                let adapter = AbaNetworkAdapter {
                    owner,
                    inner: network,
                };

                let result = aba
                    .process_message(stored, &adapter)
                    .map_err(MVBAError::Aba)?;

                let decided = matches!(result, AsyncBinaryAgreementResult::Decided);

                let outcome = match result {
                    AsyncBinaryAgreementResult::MessageQueued => MVBAResult::MessageQueued,
                    AsyncBinaryAgreementResult::MessageIgnored => MVBAResult::MessageIgnored,
                    AsyncBinaryAgreementResult::Processed | AsyncBinaryAgreementResult::Decided => {
                        MVBAResult::Processed
                    }
                };

                if decided {
                    self.finalize_aba_slot(owner)?;
                }

                if self.all_decided() {
                    return Ok(MVBAResult::Decided);
                }

                Ok(outcome)
            }
        }
    }

    fn finalize(self) -> Result<MVBAProposal, Self::Error> {
        if !self.all_decided() {
            return Err(MVBAError::NotReadyToFinalize);
        }

        let MultiValuedByzantineAgreement { abas, entries, .. } = self;

        let proposal = abas
            .into_iter()
            .filter_map(|(owner, slot)| match slot {
                AbaSlot::Done(true) => entries
                    .get(&owner)
                    .map(|(digest, sig)| (owner, *digest, sig.clone())),
                _ => None,
            })
            .collect();

        Ok(proposal)
    }
}

impl Debug for MultiValuedByzantineAgreement {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiValuedByzantineAgreement")
            .field("proposed", &self.proposed)
            .field("entries", &self.entries.len())
            .field(
                "decided",
                &self
                    .abas
                    .values()
                    .filter(|s| matches!(s, AbaSlot::Done(_)))
                    .count(),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub(crate) enum MVBAError {
    #[error("Inner ABA error: {0}")]
    Aba(ABAError),
    #[error("Attempted to finalize MVBA before all ABA instances decided")]
    NotReadyToFinalize,
}
