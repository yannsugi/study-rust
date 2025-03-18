use futures::task;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

struct Delay {
    when: Instant,
}

impl Future for Delay {
    type Output = &'static str;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<&'static str> {
        if Instant::now() >= self.when {
            println!("Hello world");
            Poll::Ready("done")
        } else {
            // [MEMO]
            // Poll::Pendingを返す場合、どこかで`waker`に対し確実に`wake`を呼び出す必要がある。
            // これを行わないと、futureは永遠に`Poll::Pending`を返し続ける。

            // 現在のタスクに紐づく "waker" ハンドルを取得
            // [MEMO]
            // `waker`は、Rustの非同期プログラミングにおいて、`Future`が再度ポーリングされることをランタイムに通知するためのハンドル。
            let waker = cx.waker().clone();
            let when = self.when;

            // タイマースレッドを spawn
            thread::spawn(move || {
                let now = Instant::now();

                if now < when {
                    thread::sleep(when - now);
                }

                // [MEMO]
                // `wake`を呼び出すことで、再度ポーリングされることを通知する。
                waker.wake();
            });

            // [MEMO]
            // `Poll::Pending`を返すことで、futureがまだ完了していないことを示す。
            // `Poll::Pending`を返すと、`waker`に対し`wake`を呼び出す必要がある。
            // `wake`は、同一コンテキストであれば、別スレッドからでも呼び出すことができる。
            Poll::Pending
        }
    }
}

// [MEMO]
// `Pin<T>`は、ある値のメモリアドレスを固定（ピン留め）することで、自己参照構造体の安全性を保証する仕組み
type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

struct MiniTokio {
    tasks: VecDeque<Task>,
}

impl MiniTokio {
    fn new() -> MiniTokio {
        MiniTokio {
            tasks: VecDeque::new(),
        }
    }

    /// mini-tokio のインスタンスに "future" を渡す
    fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.push_back(Box::pin(future));
    }

    fn run(&mut self) {
        let waker = task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // tasks が空になるまでループ
        while let Some(mut task) = self.tasks.pop_front() {
            if task.as_mut().poll(&mut cx).is_pending() {
                // pending なタスクは、再度 tasks に push しておく
                self.tasks.push_back(task);
            }
        }
    }
}

fn main() {
    let mut mini_tokio = MiniTokio::new();

    mini_tokio.spawn(async {
        let when = Instant::now() + Duration::from_millis(10);
        let future = Delay { when };

        let out = future.await;
        assert_eq!(out, "done");
    });

    mini_tokio.run();
}
