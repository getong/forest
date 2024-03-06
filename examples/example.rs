use std::{
    any::Any,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use playground::{openrpc_types::ParamStructure, SelfDescribingModule};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

trait Blockstore {
    fn get_count(&self) -> usize;
    fn increment(&self, by: usize);
}

#[derive(Default)]
struct MyBlockstore {
    count: AtomicUsize,
}

impl Blockstore for MyBlockstore {
    fn get_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn increment(&self, by: usize) {
        self.count.fetch_add(by, Ordering::SeqCst);
    }
}

struct MyCtx<BS> {
    blockstore: BS,
}

async fn count(
    ctx: Arc<MyCtx<impl Blockstore>>,
) -> Result<usize, jsonrpsee::types::ErrorObjectOwned> {
    Ok(ctx.blockstore.get_count())
}

async fn increment(
    ctx: Arc<MyCtx<impl Blockstore>>,
    by: usize,
) -> Result<(), jsonrpsee::types::ErrorObjectOwned> {
    ctx.blockstore.increment(by);
    Ok(())
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Concat {
    left: String,
    right: String,
    result: String,
}

async fn concat(
    _ctx: impl Any,
    left: String,
    right: String,
) -> Result<Concat, jsonrpsee::types::ErrorObjectOwned> {
    let result = format!("{left}{right}");
    Ok(Concat {
        left,
        right,
        result,
    })
}

fn main() {
    let calling_convention = ParamStructure::Either;

    let mut module = SelfDescribingModule::new(MyCtx {
        blockstore: MyBlockstore::default(),
    });
    module
        .serve("count", calling_convention, [], count)
        .serve("increment", calling_convention, ["amount"], increment)
        .serve("concat", calling_convention, ["left", "right"], concat);
    let (_module, doc) = module.finish();
    println!("{:#}", serde_json::to_value(doc).unwrap());
}
