extern crate exonum;
extern crate eden_exonum as cryptocurrency;
#[macro_use] extern crate exonum_testkit;

use exonum::blockchain::Transaction;
use exonum::crypto::{self, PublicKey, SecretKey};
use exonum_testkit::{TestKit, TestKitBuilder};
// Import datatypes used in tests from the crate where the service is defined.
use cryptocurrency::schema::{CurrencySchema, Wallet};
use cryptocurrency::transactions::{TxCreateWallet, TxTransfer};
use cryptocurrency::service::CurrencyService;

fn init_testkit() -> TestKit {
    TestKitBuilder::validator()
        .with_service(CurrencyService)
        .create()
}

#[test]
fn test_create_wallet() {
    let mut testkit = init_testkit();
    let (pubkey, key) = crypto::gen_keypair();
    testkit.create_block_with_transactions(txvec![
        TxCreateWallet::new(&pubkey, "Alice", &key),
    ]);
    let wallet = {
        let snapshot = testkit.snapshot();
        CurrencySchema::new(&snapshot).wallet(&pubkey).expect(
            "No wallet persisted",
        )
    };
    assert_eq!(*wallet.pub_key(), pubkey);
    assert_eq!(wallet.name(), "Alice");
    assert_eq!(wallet.balance(), 100);
}