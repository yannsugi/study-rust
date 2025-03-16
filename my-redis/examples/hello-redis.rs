use bytes::Bytes;
use mini_redis::client;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

// 複数の異なるコマンドは1つのチャネルを通して「多重化 (multiplexed)」される
#[derive(Debug)]
enum Command {
    Get {
        key: String,
        resp: Responder<Option<Bytes>>,
    },
    Set {
        key: String,
        val: Vec<u8>,
        resp: Responder<()>,
    },
}

/// リクエストを送る側が生成する。
/// "マネージャー" タスクがレスポンスをリクエスト側に送り返すために使われる
type Responder<T> = oneshot::Sender<mini_redis::Result<T>>;

#[tokio::main]
async fn main() {
    // 最大 32 のキャパシティをもったチャネルを作成
    let (tx, mut rx) = mpsc::channel(32);

    let manager = tokio::spawn(async move {
        // サーバーへのコネクションを確立する
        let mut client = client::connect("127.0.0.1:6379").await.unwrap();

        // メッセージの受信を開始
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Command::Get { key, resp } => {
                    let res = client.get(&key).await;
                    // エラーは無視する
                    let _ = resp.send(res);
                }
                Command::Set { key, val, resp } => {
                    let res = client.set(&key, val.into()).await;
                    // エラーは無視する
                    let _ = resp.send(res);
                }
            }
        }
    });

    // `Sender` ハンドルはタスクにムーブされる。
    // タスクは2つあるので、2つ目の `Sender` を作る必要がある。
    let tx2 = tx.clone();

    // タスク1は "get" を、タスク2は "set" を担当する
    let t1 = tokio::spawn(async move {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = Command::Get {
            key: "hello".to_string(),
            resp: resp_tx,
        };

        // GET リクエストを送信
        tx.send(cmd).await.unwrap();

        // レスポンスが来るのを待つ
        let res = resp_rx.await;
        println!("GOT = {:?}", res);
    });

    let t2 = tokio::spawn(async move {
        let (resp_tx, resp_rx) = oneshot::channel();
        let cmd = Command::Set {
            key: "foo".to_string(),
            val: b"bar".to_vec(),
            resp: resp_tx,
        };

        // SET リクエストを送信
        tx2.send(cmd).await.unwrap();

        // レスポンスが来るのを待つ
        let res = resp_rx.await;
        println!("GOT = {:?}", res);
    });

    // なぜ、manager の前にt1,t2があるのに処理ができるのか？
    // それは、channelのsenderのバッファにt1,t2の処理が溜め込まれるため。
    // managerの処理が始まると、senderのバッファの処理を順次処理していく。
    // そして、senderのバッファが空になるとrecv()がNoneを返すようになり、managerの処理が終了する。
    t1.await.unwrap();
    t2.await.unwrap();
    manager.await.unwrap();
}
