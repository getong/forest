use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use playground::{
    jsonrpc_types::Error, openrpc_types::ParamStructure, ConcreteCallingConvention, RpcMethod,
    RpcMethodExt, SelfDescribingRpcModule,
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
impl<BS: Send + Sync + Blockstore> RpcMethod<0, MyCtx<BS>> for Count {
    const NAME: &'static str = "count";
    const PARAM_NAMES: [&'static str; 0] = [];
    type Params = ();
    type Ok = usize;

    async fn handle(ctx: Arc<MyCtx<BS>>, (): Self::Params) -> Result<Self::Ok, Error> {
        Ok(ctx.blockstore.get_count())
    }
}

enum Increment {}
impl<BS: Send + Sync + Blockstore> RpcMethod<1, MyCtx<BS>> for Increment {
    const NAME: &'static str = "increment";
    const PARAM_NAMES: [&'static str; 1] = ["by"];
    type Params = (usize,);
    type Ok = ();

    async fn handle(ctx: Arc<MyCtx<BS>>, (by,): Self::Params) -> Result<Self::Ok, Error> {
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
impl<BS: Sync + Send> RpcMethod<2, BS> for Concat {
    const NAME: &'static str = "concat";
    const PARAM_NAMES: [&'static str; 2] = ["left", "right"];
    type Params = (String, String);
    type Ok = ConcatResult;
    async fn handle(_ctx: Arc<BS>, (left, right): Self::Params) -> Result<Self::Ok, Error> {
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
    let mut module = SelfDescribingRpcModule::new(
        MyCtx {
            blockstore: MyBlockstore::default(),
        },
        ParamStructure::Either,
    );
    Count::register(&mut module);
    Increment::register(&mut module);
    Concat::register(&mut module);
    let (module, doc) = module.finish();
    println!("{:#}", serde_json::to_value(doc).unwrap());
    dbg!(
        Concat::call(
            &module,
            ("hello".into(), "world".into()),
            ConcreteCallingConvention::ByName
        )
        .await
    )
    .unwrap();
}
