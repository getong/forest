use std::{
    future::Future,
    marker::PhantomData,
    task::{Context, Poll},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower::Service;

use crate::jsonrpc_types::{Error, RequestParameters};

pub struct Request {
    pub parameters: Option<RequestParameters>,
    pub extensions: http::Extensions,
}

pub struct Stateless;

static_assertions::assert_not_impl_any!(Stateless: Clone);

pub trait Handler<const ARITY: usize, HandlerArgsT, StateT> {
    type FutureT: Future<Output = Result<Value, Error>>;

    fn call(self, request: Request, state: StateT) -> Self::FutureT;

    fn with_state(self, state: StateT) -> HandlerService<ARITY, Self, HandlerArgsT, StateT>
    where
        Self: Sized,
    {
        HandlerService {
            handler: self,
            state,
            _handler_args: PhantomData,
        }
    }
}

impl<F, Fut, R, T0, T1> Handler<2, (T0, T1), Stateless> for F
where
    T0: for<'de> Deserialize<'de>,
    T1: for<'de> Deserialize<'de>,
    F: Fn(T0, T1) -> Fut,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type FutureT = futures::future::MapOk<Fut, fn(R) -> Value>;

    fn call(self, request: Request, state: Stateless) -> Self::FutureT {
        todo!()
    }
}

impl<F, Fut, R, StateT, T0> Handler<1, (T0,), StateT> for F
where
    T0: for<'de> Deserialize<'de>,
    F: Fn(StateT, T0) -> Fut,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
    StateT: Clone,
{
    type FutureT = futures::future::MapOk<Fut, fn(R) -> Value>;

    fn call(self, request: Request, state: StateT) -> Self::FutureT {
        todo!()
    }
}

impl<F, Fut, R, StateT, T0, T1> Handler<2, (T0, T1), StateT> for F
where
    T0: for<'de> Deserialize<'de>,
    T1: for<'de> Deserialize<'de>,
    F: Fn(StateT, T0, T1) -> Fut,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
    StateT: Clone,
{
    type FutureT = futures::future::MapOk<Fut, fn(R) -> Value>;

    fn call(self, request: Request, state: StateT) -> Self::FutureT {
        todo!()
    }
}

pub struct HandlerService<const ARITY: usize, HandlerT, HandlerArgsT, StateT> {
    handler: HandlerT,
    state: StateT,
    _handler_args: PhantomData<fn() -> (HandlerArgsT,)>,
}

impl<const ARITY: usize, HandlerT, HandlerArgsT, StateT> Service<Request>
    for HandlerService<ARITY, HandlerT, HandlerArgsT, StateT>
where
    HandlerT: Handler<ARITY, HandlerArgsT, StateT> + Clone,
    StateT: Clone + Send + Sync,
{
    type Response = Value;

    type Error = Error;

    type Future = HandlerT::FutureT;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // TODO(aatifsyed): reasoning
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        self.handler.clone().call(request, self.state.clone())
    }
}
