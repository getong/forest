use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use playground::{
    call, jsonrpc_types::Error, openrpc_types::ParamStructure, ConcreteCallingConvention,
    RpcEndpoint, SelfDescribingModule,
};
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

enum Count {}
impl<BS: Send + Sync + Blockstore> RpcEndpoint<0, Arc<MyCtx<BS>>> for Count {
    const METHOD_NAME: &'static str = "count";
    const ARG_NAMES: [&'static str; 0] = [];
    type Args = ();
    type Ok = usize;

    async fn handle(ctx: Arc<MyCtx<BS>>, (): Self::Args) -> Result<Self::Ok, Error> {
        Ok(ctx.blockstore.get_count())
    }
}

enum Increment {}
impl<BS: Send + Sync + Blockstore> RpcEndpoint<1, Arc<MyCtx<BS>>> for Increment {
    const METHOD_NAME: &'static str = "increment";
    const ARG_NAMES: [&'static str; 1] = ["by"];
    type Args = (usize,);
    type Ok = ();

    async fn handle(ctx: Arc<MyCtx<BS>>, (by,): Self::Args) -> Result<Self::Ok, Error> {
        ctx.blockstore.increment(by);
        Ok(())
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct ConcatResult {
    left: String,
    right: String,
    result: String,
    info: NestedInfo,
}
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct NestedInfo {
    x: usize,
    y: usize,
}

enum Concat {}
impl<BS: Send> RpcEndpoint<2, BS> for Concat {
    const METHOD_NAME: &'static str = "concat";
    const ARG_NAMES: [&'static str; 2] = ["left", "right"];
    type Args = (String, String);
    type Ok = ConcatResult;
    async fn handle(_ctx: BS, (left, right): Self::Args) -> Result<Self::Ok, Error> {
        let result = format!("{left}{right}");
        Ok(ConcatResult {
            left,
            right,
            result,
            info: NestedInfo { x: 1, y: 2 },
        })
    }
}

fn main() {
    futures::executor::block_on(_main());
}

async fn _main() {
    let mut module = SelfDescribingModule::new(
        MyCtx {
            blockstore: MyBlockstore::default(),
        },
        ParamStructure::Either,
    );
    module
        .register::<0, Count>()
        .register::<1, Increment>()
        .register::<2, Concat>();
    let (module, doc) = module.finish();
    println!("{:#}", serde_json::to_value(doc).unwrap());
    dbg!(
        call::<2, (), Concat>(
            &module,
            ("hello".into(), "world".into()),
            ConcreteCallingConvention::ByName
        )
        .await
    );
}
