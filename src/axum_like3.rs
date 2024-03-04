use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{ready, Context, Poll},
};

use pin_project_lite::pin_project;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower::Service;

use crate::{
    jsonrpc_types::{Error, RequestParameters},
    parser::Parser,
};

pub struct Request {
    pub parameters: Option<RequestParameters>,
    pub extensions: http::Extensions,
}

/// [`axum::handler::Handler`]
pub trait Handler<const ARITY: usize, const STATE: bool, HandlerArgsT, StateT> {
    type FutureT: Future<Output = Result<Value, Error>>;

    fn call(self, request: Request, state: StateT) -> Self::FutureT;

    fn with_state(self, state: StateT) -> HandlerService<ARITY, STATE, Self, HandlerArgsT, StateT>
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

pub trait StatelessHandlerExt<const ARITY: usize, HandlerArgsT>:
    Handler<ARITY, false, HandlerArgsT, ()>
{
    fn into_service(self) -> HandlerService<ARITY, false, Self, HandlerArgsT, ()>
    where
        Self: Sized,
    {
        self.with_state(())
    }
}

pub struct HandlerFn<'a, const ARITY: usize, F> {
    inner: F,
    names: [&'a str; ARITY],
}

impl<'a, F, Fut, R, T0, T1> Handler<2, false, (T0, T1), ()> for HandlerFn<'a, 2, F>
where
    T0: for<'de> Deserialize<'de>,
    T1: for<'de> Deserialize<'de>,
    F: FnOnce(T0, T1) -> Fut,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type FutureT = AndThenDeserializeResponse<Fut>;

    fn call(
        self,
        Request {
            parameters,
            extensions,
        }: Request,
        _: (),
    ) -> Self::FutureT {
        let Self { inner, names } = self;
        let parser = Parser::new(parameters, &names, todo!());
        todo!()
    }
}

impl<'a, F, Fut, R, StateT, T0> Handler<1, true, (T0,), StateT> for HandlerFn<'a, 1, F>
where
    T0: for<'de> Deserialize<'de>,
    F: FnOnce(StateT, T0) -> Fut,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type FutureT = AndThenDeserializeResponse<Fut>;

    fn call(self, request: Request, state: StateT) -> Self::FutureT {
        todo!()
    }
}

impl<'a, F, Fut, R, StateT, T0, T1> Handler<2, true, (T0, T1), StateT> for HandlerFn<'a, 2, F>
where
    T0: for<'de> Deserialize<'de>,
    T1: for<'de> Deserialize<'de>,
    F: FnOnce(StateT, T0, T1) -> Fut,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
    StateT: Clone,
{
    type FutureT = AndThenDeserializeResponse<Fut>;

    fn call(self, request: Request, state: StateT) -> Self::FutureT {
        todo!()
    }
}

pub struct HandlerService<const ARITY: usize, const STATE: bool, HandlerT, HandlerArgsT, StateT> {
    handler: HandlerT,
    state: StateT,
    _handler_args: PhantomData<fn() -> (HandlerArgsT,)>,
}

impl<const ARITY: usize, HandlerT, HandlerArgsT, StateT> Service<Request>
    for HandlerService<ARITY, true, HandlerT, HandlerArgsT, StateT>
where
    HandlerT: Handler<ARITY, true, HandlerArgsT, StateT> + Clone,
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

impl<const ARITY: usize, HandlerT, HandlerArgsT> Service<Request>
    for HandlerService<ARITY, false, HandlerT, HandlerArgsT, ()>
where
    HandlerT: Handler<ARITY, false, HandlerArgsT, ()> + Clone,
{
    type Response = Value;

    type Error = Error;

    type Future = HandlerT::FutureT;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // TODO(aatifsyed): reasoning
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        self.handler.clone().call(request, ())
    }
}

pin_project! {
    pub struct AndThenDeserializeResponse<F> {
        #[pin]
        inner: F
    }
}

impl<F> AndThenDeserializeResponse<F> {
    fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<R, F> Future for AndThenDeserializeResponse<F>
where
    F: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type Output = Result<Value, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(
            serde_json::to_value(ready!(self.project().inner.poll(cx))?).map_err(|e| {
                Error::internal_error(
                    "error deserializing return value for handler",
                    json!({
                        "type": std::any::type_name::<R>(),
                        "error": e.to_string()
                    }),
                )
            }),
        )
    }
}
