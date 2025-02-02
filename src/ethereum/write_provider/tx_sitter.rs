use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::U256;
use tx_sitter_client::data::{SendTxRequest, TransactionPriority, TxStatus};
use tx_sitter_client::TxSitterClient;

use super::inner::{Inner, TransactionResult};
use super::options::TxSitterOptions;
use crate::ethereum::write::TransactionId;
use crate::ethereum::TxError;

const MINING_TIMEOUT: Duration = Duration::from_secs(60);

pub struct TxSitter {
    client:    TxSitterClient,
    gas_limit: Option<u64>,
}

impl TxSitter {
    pub fn new(options: &TxSitterOptions) -> Self {
        Self {
            client:    TxSitterClient::new(&options.tx_sitter_url),
            gas_limit: options.tx_sitter_gas_limit,
        }
    }

    pub async fn mine_transaction_inner(
        &self,
        tx_id: TransactionId,
    ) -> Result<TransactionResult, TxError> {
        loop {
            let tx = self.client.get_tx(&tx_id.0).await.map_err(TxError::Send)?;

            if tx.status == TxStatus::Mined || tx.status == TxStatus::Finalized {
                return Ok(TransactionResult {
                    transaction_id: tx.tx_id,
                    hash:           Some(
                        tx.tx_hash
                            .context("Missing hash on a mined tx")
                            .map_err(TxError::Send)?,
                    ),
                });
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

#[async_trait]
impl Inner for TxSitter {
    async fn send_transaction(
        &self,
        mut tx: TypedTransaction,
        _only_once: bool,
    ) -> Result<TransactionId, TxError> {
        if let Some(gas_limit) = self.gas_limit {
            tx.set_gas(gas_limit);
        }

        // TODO: Handle only_once
        let tx = self
            .client
            .send_tx(&SendTxRequest {
                to:        *tx
                    .to_addr()
                    .context("Tx receiver must be an address")
                    .map_err(TxError::Send)?,
                value:     tx.value().copied().unwrap_or(U256::zero()),
                data:      tx.data().cloned(),
                gas_limit: *tx
                    .gas()
                    .context("Missing tx gas limit")
                    .map_err(TxError::Send)?,
                priority:  TransactionPriority::Regular,
                tx_id:     None,
            })
            .await
            .map_err(TxError::Send)?;

        Ok(TransactionId(tx.tx_id))
    }

    async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        let unsent_txs = self
            .client
            .get_txs(Some(TxStatus::Unsent))
            .await
            .map_err(TxError::Send)?;

        let pending_txs = self
            .client
            .get_txs(Some(TxStatus::Pending))
            .await
            .map_err(TxError::Send)?;

        let mut txs = vec![];

        for tx in unsent_txs.into_iter().chain(pending_txs) {
            txs.push(TransactionId(tx.tx_id));
        }

        Ok(txs)
    }

    async fn mine_transaction(&self, tx: TransactionId) -> Result<TransactionResult, TxError> {
        tokio::time::timeout(MINING_TIMEOUT, self.mine_transaction_inner(tx))
            .await
            .map_err(|_| TxError::ConfirmationTimeout)?
    }
}
