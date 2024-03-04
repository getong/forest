use std::{
    future::{ready, Future, Ready},
    marker::PhantomData,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures::future::Either;
use pin_project_lite::pin_project;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower::Service;

use crate::{
    jsonrpc_types::{Error, RequestParameters},
    openrpc_types::ParamStructure,
    parser::{check_args, Parser},
    util::Optional as _,
};

#[derive(Clone)]
struct State<T = ()>(pub T);

struct ServiceFn<'a, const ARITY: usize, HandlerT, HandlerArgsT> {
    handler: HandlerT,
    param_names: [&'a str; ARITY],
    calling_convention: ParamStructure,
    _args: PhantomData<fn() -> HandlerArgsT>,
}

impl<'a, const ARITY: usize, HandlerT, HandlerArgsT> ServiceFn<'a, ARITY, HandlerT, HandlerArgsT> {
    pub fn new(
        handler: HandlerT,
        param_names: [&'a str; ARITY],
        calling_convention: ParamStructure,
    ) -> Self
    where
        HandlerT: CheckArgs<ARITY, HandlerArgsT>,
    {
        check_args(param_names, HandlerT::optional());
        Self {
            handler,
            param_names,
            calling_convention,
            _args: PhantomData,
        }
    }
}

trait CheckArgs<const ARITY: usize, Args> {
    fn optional() -> [bool; ARITY];
}

impl<F, R> CheckArgs<0, ()> for F
where
    F: FnOnce() -> R,
{
    fn optional() -> [bool; 0] {
        []
    }
}

impl<'de, F, T0, R> CheckArgs<1, (T0,)> for F
where
    F: FnOnce(T0) -> R,
    T0: Deserialize<'de>,
{
    fn optional() -> [bool; 1] {
        [T0::optional()]
    }
}

impl<'de, F, T0, T1, R> CheckArgs<2, (T0, T1)> for F
where
    F: FnOnce(T0, T1) -> R,
    T0: Deserialize<'de>,
    T1: Deserialize<'de>,
{
    fn optional() -> [bool; 2] {
        [T0::optional(), T1::optional()]
    }
}

impl<'de, F, T0, T1, T2, R> CheckArgs<3, (T0, T1, T2)> for F
where
    F: FnOnce(T0, T1, T2) -> R,
    T0: Deserialize<'de>,
    T1: Deserialize<'de>,
    T2: Deserialize<'de>,
{
    fn optional() -> [bool; 3] {
        [T0::optional(), T1::optional(), T2::optional()]
    }
}

impl<const ARITY: usize, HandlerT, HandlerArgsT> ServiceFn<'_, ARITY, HandlerT, HandlerArgsT> {
    fn parser(&self, params: Option<RequestParameters>) -> Result<Parser<'_>, Error> {
        Parser::new(params, &self.param_names, self.calling_convention)
    }
}

impl<'a, HandlerT, Fut, R> Service<Option<RequestParameters>> for ServiceFn<'a, 0, HandlerT, ()>
where
    HandlerT: FnMut() -> Fut,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type Response = Value;

    type Error = Error;

    type Future = Wrapped<Fut>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Option<RequestParameters>) -> Self::Future {
        let _ = tri!(self.parser(req));
        Wrapped::cont((self.handler)())
    }
}

impl<'a, F, T0, Fut, R> Service<Option<RequestParameters>> for ServiceFn<'a, 1, F, (T0,)>
where
    F: FnMut(T0) -> Fut,
    T0: for<'de> Deserialize<'de>,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type Response = Value;

    type Error = Error;

    type Future = Wrapped<Fut>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Option<RequestParameters>) -> Self::Future {
        let mut parser = tri!(self.parser(req));
        let t0 = tri!(parser.parse());
        drop(parser);
        Wrapped::cont((self.handler)(t0))
    }
}

impl<'a, F, T0, T1, Fut, R> Service<Option<RequestParameters>> for ServiceFn<'a, 2, F, (T0, T1)>
where
    F: FnMut(T0, T1) -> Fut,
    T0: for<'de> Deserialize<'de>,
    T1: for<'de> Deserialize<'de>,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type Response = Value;

    type Error = Error;

    type Future = Wrapped<Fut>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Option<RequestParameters>) -> Self::Future {
        let mut parser = tri!(self.parser(req));
        let t0 = tri!(parser.parse());
        let t1 = tri!(parser.parse());
        drop(parser);
        Wrapped::cont((self.handler)(t0, t1))
    }
}

impl<'a, F, T0, T1, T2, Fut, R> Service<Option<RequestParameters>>
    for ServiceFn<'a, 3, F, (T0, T1, T2)>
where
    F: FnMut(T0, T1, T2) -> Fut,
    T0: for<'de> Deserialize<'de>,
    T1: for<'de> Deserialize<'de>,
    T2: for<'de> Deserialize<'de>,
    Fut: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type Response = Value;

    type Error = Error;

    type Future = Wrapped<Fut>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Option<RequestParameters>) -> Self::Future {
        let mut parser = tri!(self.parser(req));
        let t0 = tri!(parser.parse());
        let t1 = tri!(parser.parse());
        let t2 = tri!(parser.parse());
        drop(parser);
        Wrapped::cont((self.handler)(t0, t1, t2))
    }
}

macro_rules! tri {
    ($expr:expr) => {
        match $expr {
            Ok(it) => it,
            Err(e) => return Wrapped::stop(e),
        }
    };
}
pub(crate) use tri;

pin_project! {
    pub struct Wrapped<F> {
        #[pin]
        inner: Either<Ready<Result<Value, Error>>, AndThenDeserializeResponse<F>>,
    }
}

impl<F> Wrapped<F> {
    fn stop(error: Error) -> Self {
        Self {
            inner: Either::Left(ready(Err(error))),
        }
    }
    fn cont(job: F) -> Self {
        Self {
            inner: Either::Right(AndThenDeserializeResponse { inner: job }),
        }
    }
}

impl<R, F> Future for Wrapped<F>
where
    F: Future<Output = Result<R, Error>>,
    R: Serialize,
{
    type Output = Result<Value, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

pin_project! {
    pub struct AndThenDeserializeResponse<F> {
        #[pin]
        inner: F
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::util::{call, examples, from_value};
}
