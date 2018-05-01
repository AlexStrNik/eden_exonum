#![allow(missing_docs)]

extern crate bodyparser;
#[macro_use]
extern crate exonum;
#[macro_use]
extern crate failure;
extern crate iron;
extern crate router;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;


pub mod schema {
    use exonum::storage::{Fork, MapIndex, Snapshot};
    use exonum::crypto::PublicKey;

    encoding_struct! {
        struct Wallet {
            pub_key: &PublicKey,
            name: &str,
            email: &str,
            balance: u64,
        }
    }

    impl Wallet {
        pub fn increase(self, amount: u64) -> Self {
            let balance = self.balance() + amount;
            Self::new(self.pub_key(), self.name(), self.email(), balance)
        }

        pub fn decrease(self, amount: u64) -> Self {
            let balance = self.balance() - amount;
            Self::new(self.pub_key(), self.name(), self.email(), balance)
        }

        pub fn freeze(self, amount: u64) -> Self {
            let balance = self.balance() - amount;
            Self::new(self.pub_key(), self.name(), self.email(), balance)
        }
    }

    pub struct CurrencySchema<T> {
        view: T,
    }

    impl<T: AsRef<Snapshot>> CurrencySchema<T> {
        pub fn new(view: T) -> Self {
            CurrencySchema { view }
        }

        pub fn wallets(&self) -> MapIndex<&Snapshot, PublicKey, Wallet> {
            MapIndex::new("cryptocurrency.wallets", self.view.as_ref())
        }

        // Utility method to quickly get a separate wallet from the storage
        pub fn wallet(&self, pub_key: &PublicKey) -> Option<Wallet> {
            self.wallets().get(pub_key)
        }
    }

    impl<'a> CurrencySchema<&'a mut Fork> {
        pub fn wallets_mut(&mut self) -> MapIndex<&mut Fork, PublicKey, Wallet> {
            MapIndex::new("cryptocurrency.wallets", &mut self.view)
        }
    }
}

pub mod transactions {
    use exonum::crypto::PublicKey;

    use service::SERVICE_ID;

    transactions! {
        // Transaction group.
        pub CurrencyTransactions {
            const SERVICE_ID = SERVICE_ID;

            // Transaction type for creating a new wallet.
            struct TxCreateWallet {
                pub_key: &PublicKey,
                name: &str,
                email: &str,
            }

            // Transaction type for transferring tokens between two wallets.
            struct TxTransfer {
                from: &PublicKey,
                to: &PublicKey,
                amount: u64,
                seed: u64,
            }

            struct TxFreeze {
                pub_key: &PublicKey,
                amount: u64,
            }
        }
    }
}

pub mod errors {
    use exonum::blockchain::ExecutionError;

    #[derive(Debug, Fail)]
    #[repr(u8)]
    pub enum Error {
        #[fail(display = "Wallet already exists")]
        WalletAlreadyExists = 0,

        #[fail(display = "Sender doesn't exist")]
        SenderNotFound = 1,

        #[fail(display = "Receiver doesn't exist")]
        ReceiverNotFound = 2,

        #[fail(display = "Insufficient currency amount")]
        InsufficientCurrencyAmount = 3,
    }

    // Conversion between service-specific errors and the standard error type
// that can be emitted by transactions.
    impl From<Error> for ExecutionError {
        fn from(value: Error) -> ExecutionError {
            let description = format!("{}", value);
            ExecutionError::with_description(value as u8, description)
        }
    }
}

pub mod contracts {
    use exonum::blockchain::{ExecutionResult, Transaction};
    use exonum::{messages::Message, storage::Fork};

    use schema::{CurrencySchema, Wallet};
    use transactions::{TxCreateWallet, TxTransfer, TxFreeze};
    use errors::Error;

    const INIT_BALANCE: u64 = 0;


    impl Transaction for TxCreateWallet {
        fn verify(&self) -> bool {
            self.verify_signature(self.pub_key())
        }

        fn execute(&self, view: &mut Fork) -> ExecutionResult {
            let mut schema = CurrencySchema::new(view);
            if schema.wallet(self.pub_key()).is_none() {
                let wallet = Wallet::new(self.pub_key(), self.name(), self.email(), INIT_BALANCE);
                println!("Create the wallet: {:?}", wallet);
                schema.wallets_mut().put(self.pub_key(), wallet);
                Ok(())
            } else {
                Err(Error::WalletAlreadyExists)?
            }
        }
    }

    impl Transaction for TxTransfer {
        fn verify(&self) -> bool {
            (*self.from() != *self.to()) &&
                self.verify_signature(self.from())
        }

        fn execute(&self, view: &mut Fork) -> ExecutionResult {
            let mut schema = CurrencySchema::new(view);

            let sender = match schema.wallet(self.from()) {
                Some(val) => val,
                None => Err(Error::SenderNotFound)?,
            };

            let receiver = match schema.wallet(self.to()) {
                Some(val) => val,
                None => Err(Error::ReceiverNotFound)?,
            };

            let amount = self.amount();
            if sender.balance() >= amount {
                let sender = sender.decrease(amount);
                let receiver = receiver.increase(amount);
                println!("Transfer between wallets: {:?} => {:?}", sender, receiver);
                let mut wallets = schema.wallets_mut();
                wallets.put(self.from(), sender);
                wallets.put(self.to(), receiver);
                Ok(())
            } else {
                Err(Error::InsufficientCurrencyAmount)?
            }
        }
    }

    impl Transaction for TxFreeze {
        fn verify(&self) -> bool {
            self.verify_signature(self.pub_key())
        }

        fn execute(&self, view: &mut Fork) -> ExecutionResult {
            let mut schema = CurrencySchema::new(view);

            let freezer = match schema.wallet(self.pub_key()) {
                Some(val) => val,
                None => Err(Error::SenderNotFound)?,
            };

            let amount = self.amount();
            if freezer.balance() >= amount {
                let freezer = freezer.freeze(amount);
                println!("Hold {} tokens of wallet: {:?}", amount, freezer);
                let mut wallets = schema.wallets_mut();
                wallets.put(self.pub_key(), freezer);
                Ok(())
            } else {
                Err(Error::InsufficientCurrencyAmount)?
            }
        }
    }
}

pub mod api {
    use exonum::blockchain::{Blockchain, Transaction};
    use exonum::encoding::serialize::FromHex;
    use exonum::node::{ApiSender, TransactionSend};
    use exonum::crypto::{Hash, PublicKey};
    use exonum::api::{Api, ApiError};
    use iron::prelude::*;
    use iron::{headers::ContentType, modifiers::Header, status::Status};
    use router::Router;

    use bodyparser;
    use serde_json;
    use schema::{CurrencySchema, Wallet};
    use transactions::CurrencyTransactions;

    #[derive(Clone)]
    pub struct CryptocurrencyApi {
        channel: ApiSender,
        blockchain: Blockchain,
    }

    impl CryptocurrencyApi {
        /// Method for struct construction.
        pub fn new(channel: ApiSender, blockchain: Blockchain) -> CryptocurrencyApi {
            CryptocurrencyApi {
                channel,
                blockchain,
            }
        }
    }

    #[derive(Serialize, Deserialize)]
    pub struct TransactionResponse {
        // Hash of the transaction.
        pub tx_hash: Hash,
    }

    impl CryptocurrencyApi {
        fn post_transaction(&self, req: &mut Request) -> IronResult<Response> {
            match req.get::<bodyparser::Struct<CurrencyTransactions>>() {
                Ok(Some(transaction)) => {
                    let transaction: Box<Transaction> = transaction.into();
                    let tx_hash = transaction.hash();
                    self.channel.send(transaction).map_err(ApiError::from)?;
                    let json = TransactionResponse { tx_hash };
                    self.ok_response(&serde_json::to_value(&json).unwrap())
                }
                Ok(None) => Err(ApiError::BadRequest("Empty request body".into()))?,
                Err(e) => Err(ApiError::BadRequest(e.to_string()))?,
            }
        }
    }

    impl CryptocurrencyApi {
        fn get_wallet(&self, req: &mut Request) -> IronResult<Response> {
            let path = req.url.path();
            let wallet_key = path.last().unwrap();
            let public_key = PublicKey::from_hex(wallet_key).map_err(|e| {
                IronError::new(
                    e,
                    (
                        Status::BadRequest,
                        Header(ContentType::json()),
                        "\"Invalid request param: `pub_key`\"",
                    ),
                )
            })?;

            let snapshot = self.blockchain.snapshot();
            let schema = CurrencySchema::new(snapshot);

            if let Some(wallet) = schema.wallet(&public_key) {
                self.ok_response(&serde_json::to_value(wallet).unwrap())
            } else {
                self.not_found_response(
                    &serde_json::to_value("Wallet not found").unwrap()
                )
            }
        }

        fn get_wallets(&self, _: &mut Request) -> IronResult<Response> {
            let snapshot = self.blockchain.snapshot();
            let schema = CurrencySchema::new(snapshot);
            let idx = schema.wallets();
            let wallets: Vec<Wallet> = idx.values().collect();

            self.ok_response(&serde_json::to_value(&wallets).unwrap())
        }
    }

    impl Api for CryptocurrencyApi {
        fn wire(&self, router: &mut Router) {
            let self_ = self.clone();
            let post_create_wallet = move |req: &mut Request| self_.post_transaction(req);
            let self_ = self.clone();
            let post_transfer = move |req: &mut Request| self_.post_transaction(req);
            let self_ = self.clone();
            let post_freeze = move |req: &mut Request| self_.post_transaction(req);
            let self_ = self.clone();
            let get_wallets = move |req: &mut Request| self_.get_wallets(req);
            let self_ = self.clone();
            let get_wallet = move |req: &mut Request| self_.get_wallet(req);

            // Bind handlers to specific routes.
            router.post("/v1/wallets", post_create_wallet, "post_create_wallet");
            router.post("/v1/wallets/transfer", post_transfer, "post_transfer");
            router.post("/v1/wallets/freeze", post_freeze, "post_freeze");
            router.get("/v1/wallets", get_wallets, "get_wallets");
            router.get("/v1/wallet/:pub_key", get_wallet, "get_wallet");
        }
    }
}

pub mod service {
    use exonum::blockchain::{ApiContext, Service, Transaction, TransactionSet};
    use exonum::{encoding, api::Api, crypto::Hash, messages::RawTransaction, storage::Snapshot};
    use iron::Handler;
    use router::Router;

    use transactions::CurrencyTransactions;
    use api::CryptocurrencyApi;

    pub const SERVICE_ID: u16 = 1;

    pub struct CurrencyService;

    impl Service for CurrencyService {
        fn service_id(&self) -> u16 { SERVICE_ID }

        fn service_name(&self) -> &'static str { "cryptocurrency" }

        fn state_hash(&self, _: &Snapshot) -> Vec<Hash> {
            vec![]
        }

        fn tx_from_raw(&self, raw: RawTransaction) -> Result<Box<Transaction>, encoding::Error> {
            let tx = CurrencyTransactions::tx_from_raw(raw)?;
            Ok(tx.into())
        }

        fn public_api_handler(&self, ctx: &ApiContext) -> Option<Box<Handler>> {
            let mut router = Router::new();
            let api = CryptocurrencyApi::new(
                ctx.node_channel().clone(),
                ctx.blockchain().clone()
            );
            api.wire(&mut router);
            Some(Box::new(router))
        }
    }
}