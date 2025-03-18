use crossbeam::channel;
use futures::task::{self, ArcWake};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};
struct Delay {
    when: Instant,
    // [MEMO]
    // `Future` トレイトを実装する型が、`waker`を保持することで、複数のスレッドから`wake`を呼び出す場合、最新の`waker`に更新することができる。
    waker: Option<Arc<Mutex<Waker>>>,
}

impl Future for Delay {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // まず、これが "future" の初めての呼び出しであるならば、タイマースレッドを spawn する
        // もしすでにタイマースレッドが実行されているなら、保存されている `Waker` が
        // 現在のタスクの "waker" と一致することを確認する
        if let Some(waker) = &self.waker {
            let mut waker = waker.lock().unwrap();

            // 保存されている "waker" が現在のタスクの "waker" と一致するか確認する
            // この確認が必要となるのは、`Delay` インスタンスが複数回の `poll` 呼び出しで異なるタスクへとムーブする可能性があるためである
            // ムーブが発生している場合、与えられた `Context` に含まれる "waker" は別物になるため、
            // その変更を反映するように保存されている "waker" を更新しなければならない
            // [MEMO]
            // `will_wake`は、`Waker`が同一タスクを`wake`する`Waker`であるかを確認するメソッド。
            if !waker.will_wake(cx.waker()) {
                *waker = cx.waker().clone();
            }
        } else {
            // [MEMO]
            // Poll::Pendingを返す場合、どこかで`waker`に対し確実に`wake`を呼び出す必要がある。
            // これを行わないと、futureは永遠に`Poll::Pending`を返し続ける。

            // 現在のタスクに紐づく "waker" ハンドルを取得
            // [MEMO]
            // `waker`は、Rustの非同期プログラミングにおいて、`Future`が再度ポーリングされることをランタイムに通知するためのハンドル。
            let when = self.when;
            let waker = Arc::new(Mutex::new(cx.waker().clone()));
            self.waker = Some(waker.clone());

            // これは `poll` の初回呼び出しである
            // タイマースレッドを spawn する
            thread::spawn(move || {
                let now = Instant::now();

                if now < when {
                    thread::sleep(when - now);
                }

                // 指定した時間が経過した。
                // "waker" を呼び出すことで呼び出し側へと通知する

                // [MEMO]
                // cxで、`Task`の`waker`を取得しているため、`wake`を呼び出すことで、`ArcWake`の`wake_by_ref`が呼び出される。
                // `wake`を呼び出すことで、再度ポーリングされることを通知する。
                // waker.wake();

                let waker = waker.lock().unwrap();
                waker.wake_by_ref();
            });
        }

        // "waker" が保存され、タイマースレッドがスタートしたら、delay が完了したかどうかをチェックする。
        // そのためには、現在の instant を確認すればよい。もし指定時間が経過しているなら、
        // "future" は完了しているので、`Poll::Ready` を返す
        if Instant::now() >= self.when {
            Poll::Ready(())
        } else {
            // 指定時間が経過していなかった場合、"future" は未完了のため、`Poll::Pending` を返す。
            //
            // `Future` トレイトによる契約によって、`Pending` が返されるときには、
            // "future" が再度ポーリングされるべき状況になったときに "waker" へと確実に合図を送らなければならない。
            // 我々のケースでは、ここで `Pending` を返すことによって、指定された時間が経過したタイミングで `Context` 引数がもっている "waker" を呼び起こす、ということを約束していることになる。
            // 上で spawn したタイマースレッドによって、このことが保証されている。
            //
            // もし "waker" を呼び起こすのを忘れたら、タスクは永遠に完了しない。
            // [MEMO]
            // `Poll::Pending`を返すことで、futureがまだ完了していないことを示す。
            // `Poll::Pending`を返すと、`waker`に対し`wake`を呼び出す必要がある。
            // `wake`は、同一コンテキストであれば、別スレッドからでも呼び出すことができる。

            // [MEMO]
            // 別スレッドで指定時間sleepして次回はnow >= whenの条件になることが確定するので、ここでwakeを呼び出す必要がない。
            Poll::Pending
        }
    }
}

struct Task {
    // `Task` が `Sync` であるようにするため、`Mutex` を利用します。
    // 任意のタイミングで、`future` にアクセスするスレッドがただ1つであることが保証されます。
    // `Mutex` は正しい実装のために必須であるわけではありません。
    // 実際、本物の Tokio はここで mutex を利用せず、より多くの行数を費やして処理しています
    // （チュートリアルには収まらない量です）
    // [MEMO]
    // `Pin<T>`は、ある値のメモリアドレスを固定（ピン留め）することで、自己参照構造体の安全性を保証する仕組み
    future: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
    executor: channel::Sender<Arc<Task>>,
}

// [MEMO]
// `ArcWake`は、Rustの非同期プログラミングにおいて、`Waker`を手動管理するための仕組み。
// `wake`時に呼びだされる関数を定義するためのトレイト。
impl ArcWake for Task {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        // [MEMO]
        // `MniTokio`の`Sender`に対して、`Task`を送信することで、再度スケジューリングを行う。
        arc_self.schedule();
    }
}

impl Task {
    fn schedule(self: &Arc<Self>) {
        let _ = self.executor.send(self.clone());
    }

    fn poll(self: Arc<Self>) {
        // `Task` インスタンスから "waker" を生成する
        // 上で実装した `ArcWake` を利用する
        let waker = task::waker(self.clone());
        let mut cx: Context<'_> = Context::from_waker(&waker);

        // 別のスレッドは "future" のロックを取ろうとしていない
        let mut future = self.future.try_lock().unwrap();

        // "future" をポーリングする
        // [MEMO]
        // こちらは`Future` トレイトのpollメソッドを呼び出している
        let _ = future.as_mut().poll(&mut cx);
    }

    // 与えられた "future" に関する新しいタスクを spawn する
    //
    // "future" を含むタスクを新しく作り、`sender` にプッシュする
    // チャネルの受信側はタスクを取得して実行する
    // [MEMO]
    // `MiniTokio`の`Sender`を引数に取ることで、`MiniTokio`のインスタンスに対して`Task`を送信することができる
    fn spawn<F>(future: F, sender: &channel::Sender<Arc<Task>>)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let task = Arc::new(Task {
            future: Mutex::new(Box::pin(future)),
            executor: sender.clone(),
        });

        let _ = sender.send(task);
    }
}

struct MiniTokio {
    scheduled: channel::Receiver<Arc<Task>>,
    sender: channel::Sender<Arc<Task>>,
}

impl MiniTokio {
    /// mini-tokio インスタンスを初期化する
    fn new() -> MiniTokio {
        let (sender, scheduled) = channel::unbounded();

        MiniTokio { scheduled, sender }
    }

    fn run(&self) {
        while let Ok(task) = self.scheduled.recv() {
            task.poll();
        }
    }

    /// mini-tokio のインスタンスに "future" を渡す
    ///
    /// 与えられる "future" は `Task` によってラップされ、`スケジュール` キューにプッシュされる。
    /// `run` が呼び出されたときに "future" が実行される
    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        Task::spawn(future, &self.sender);
    }
}

fn main() {
    let mini_tokio = MiniTokio::new();

    mini_tokio.spawn(async {
        let when = Instant::now() + Duration::from_millis(10);
        let future = Delay { when, waker: None };

        let out = future.await;
        assert_eq!(out, ());
    });

    mini_tokio.run();
}

// Notifyを使うと`waker`に関する詳細をよしなに処理してくれる。
// use tokio::sync::Notify;
// use std::sync::Arc;
// use std::time::{Duration, Instant};
// use std::thread;

// async fn delay(dur: Duration) {
//     let when = Instant::now() + dur;
//     let notify = Arc::new(Notify::new());
//     let notify2 = notify.clone();

//     thread::spawn(move || {
//         let now = Instant::now();

//         if now < when {
//             thread::sleep(when - now);
//         }

//         notify2.notify_one();
//     });

//     notify.notified().await;
// }
