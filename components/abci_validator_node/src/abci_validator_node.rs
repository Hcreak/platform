#![deny(warnings)]
use abci::*;
use ledger::data_model::errors::PlatformError;
use ledger::store::*;
use ledger_api_service::RestfulApiService;
use rand_chacha::ChaChaRng;
use rand_core::SeedableRng;
use std::sync::{Arc, RwLock};
use std::thread;
use submission_server::{convert_tx, SubmissionServer};

struct ABCISubmissionServer {
  la: SubmissionServer<ChaChaRng, LedgerState>,
}

impl ABCISubmissionServer {
  fn new() -> Result<ABCISubmissionServer, PlatformError> {
    let ledger = LedgerState::test_ledger();
    let prng = rand_chacha::ChaChaRng::from_entropy();
    Ok(ABCISubmissionServer { la: SubmissionServer::new(prng, Arc::new(RwLock::new(ledger)), 8)? })
  }
}

// TODO: implement abci hooks
impl abci::Application for ABCISubmissionServer {
  fn check_tx(&mut self, req: &RequestCheckTx) -> ResponseCheckTx {
    // Get the Tx [u8] and convert to u64
    let mut resp = ResponseCheckTx::new();

    if let Some(tx) = convert_tx(req.get_tx()) {
      if let Ok(mut state) = self.la.get_committed_state().write() {
        if TxnEffect::compute_effect(state.get_prng(), tx).is_err() {
          resp.set_code(1);
          resp.set_log(String::from("Check failed"));
        }
      } else {
        resp.set_code(1);
        resp.set_log(String::from("Could not access ledger"));
      }
    } else {
      resp.set_code(1);
      resp.set_log(String::from("Could not unpack transaction"));
    }

    resp
  }

  fn deliver_tx(&mut self, req: &RequestDeliverTx) -> ResponseDeliverTx {
    // Get the Tx [u8]
    let mut resp = ResponseDeliverTx::new();
    if let Some(tx) = convert_tx(req.get_tx()) {
      if self.la.cache_transaction(tx).is_ok() {
        return resp;
      }
    }
    resp.set_code(1);
    resp
  }

  fn begin_block(&mut self, _req: &RequestBeginBlock) -> ResponseBeginBlock {
    self.la.begin_block();
    ResponseBeginBlock::new()
  }

  fn end_block(&mut self, _req: &RequestEndBlock) -> ResponseEndBlock {
    // TODO: this should propagate errors instead of panicking
    self.la.end_block().unwrap();
    ResponseEndBlock::new()
  }

  fn commit(&mut self, _req: &RequestCommit) -> ResponseCommit {
    self.la.begin_commit();
    // TODO: anything not handled by the general SubmissionServer (publishing notifications?) should go here.
    self.la.end_commit();
    ResponseCommit::new()
  }

  fn query(&mut self, req: &RequestQuery) -> ResponseQuery {
    println!("{:?}", &req);
    let q = &req.data;
    println!("Path = {}, data = {:?}", &req.path, q);
    ResponseQuery::new()
  }
}

fn main() {
  // Tendermint ABCI port
  // let addr = "127.0.0.1:26658".parse().unwrap();

  // abci::run(addr, ABCISubmissionServer::default());
  let app = ABCISubmissionServer::new().unwrap();
  let ledger_state = app.la.borrowable_ledger_state();
  let host = std::option_env!("SERVER_HOST").unwrap_or("localhost");
  let port = std::option_env!("SERVER_PORT").unwrap_or("8668");
  let _join = thread::spawn(move || {
    let query_service = RestfulApiService::create(ledger_state, host, port)?;
    query_service.run()
  });

  abci::run_local(app);
}
