// Copyright 2023 Greptime Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use ratelimit::Ratelimiter;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio::time::Sleep;

enum State {
    Pending,
    Backoff(Pin<Box<Sleep>>),
}

#[pin_project::pin_project]
struct ThrottledFuture<T> {
    #[pin]
    inner: T,
    state: State,
    limiter: Arc<Ratelimiter>,
}

impl<T> ThrottledFuture<T> {
    pub(crate) fn new(f: T, limiter: Arc<Ratelimiter>) -> Self {
        Self {
            inner: f,
            state: State::Pending,
            limiter,
        }
    }
}

impl<T, O> Future for ThrottledFuture<T>
where
    T: Future<Output = O>,
{
    type Output = O;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.state {
            State::Pending => {
                if let Err(d) = this.limiter.try_wait() {
                    *this.state = State::Backoff(Box::pin(tokio::time::sleep(d)));
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                };
                match this.inner.poll(cx) {
                    Poll::Ready(result) => Poll::Ready(result),
                    Poll::Pending => {
                        *this.state = State::Pending;
                        Poll::Pending
                    }
                }
            }
            State::Backoff(sleep) => match sleep.as_mut().poll(cx) {
                Poll::Ready(_) => {
                    *this.state = State::Pending;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

pub struct ThrottledRuntime {
    runtime: Runtime,
    ratelimit: Arc<Ratelimiter>,
}

impl ThrottledRuntime {
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime
            .spawn(ThrottledFuture::new(future, self.ratelimit.clone()))
    }
}
