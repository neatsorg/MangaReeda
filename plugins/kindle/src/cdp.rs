//! Chrome DevTools Protocol の最小限の取り次ぎ。ブラウザ全体の WebSocket を1本だけ開き、
//! ページへは flatten したセッション（sessionId）で命令を送る。

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// 1つの命令の答えを待つ時間の上限
const CALL_LIMIT: Duration = Duration::from_secs(30);
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub struct Cdp {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    /// 答えを待つあいだに届いたイベント
    pending: VecDeque<Value>,
    /// ブラウザとの接続が切れたか（ブラウザが終わったか落ちた）。切れたら、つなぎ直すしかない
    lost: bool,
}

/// CDP から届いたイベント
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

impl Cdp {
    pub fn connect(url: &str) -> Result<Self, String> {
        let (socket, _) =
            tungstenite::connect(url).map_err(|e| format!("DevTools に接続できません（{url}）: {e}"))?;
        Ok(Self {
            socket,
            next_id: 1,
            pending: VecDeque::new(),
            lost: false,
        })
    }

    /// ブラウザとの接続が切れたか
    pub fn is_lost(&self) -> bool {
        self.lost
    }

    /// 接続が切れたことを覚えて、そのことを伝える文を返す
    fn lose(&mut self, message: String) -> String {
        self.lost = true;
        message
    }

    fn read(&mut self) -> Result<Value, String> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(text.as_str())
                        .map_err(|e| format!("DevTools の答えを読めません: {e}"));
                }
                Ok(Message::Close(_)) => return Err(self.lose("ブラウザとの接続が閉じました".into())),
                Ok(_) => continue,
                // スリープから戻ったときなどに、読むのが中断されただけ
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(self.lose(format!("ブラウザとの接続が切れました: {e}"))),
            }
        }
    }

    /// 命令を送り、答えを待つ。session が None ならブラウザ全体への命令
    pub fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::text(message.to_string()))
            .map_err(|e| format!("DevTools に送れません: {e}"))
            .map_err(|e| self.lose(e))?;
        // ブラウザが答えなくなっても止まったままにならないよう、待つ時間に上限を設ける
        // （止まっていると、本体からの終わりの頼みも受け取れない）
        let deadline = Instant::now() + CALL_LIMIT;
        loop {
            let reply = match self.read_within(deadline)? {
                Some(reply) => reply,
                None => return Err(format!("{method} の答えが来ません")),
            };
            if reply.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = reply.get("error") {
                    return Err(format!("{method} が失敗しました: {error}"));
                }
                return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
            }
            if reply.get("method").is_some() {
                self.pending.push_back(reply);
            }
        }
    }

    /// `deadline` までに届いた1通。届かなければ None
    fn read_within(&mut self, deadline: Instant) -> Result<Option<Value>, String> {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            self.set_read_timeout(Some((deadline - now).min(Duration::from_millis(500))));
            let read = self.socket.read();
            self.set_read_timeout(None);
            match read {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str(text.as_str())
                        .map(Some)
                        .map_err(|e| format!("DevTools の答えを読めません: {e}"));
                }
                Ok(Message::Close(_)) => return Err(self.lose("ブラウザとの接続が閉じました".into())),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
                Err(e) => return Err(self.lose(format!("ブラウザとの接続が切れました: {e}"))),
            }
        }
    }

    /// 次のイベントを待つ
    pub fn next_event(&mut self) -> Result<Event, String> {
        loop {
            let message = match self.pending.pop_front() {
                Some(message) => message,
                None => self.read()?,
            };
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            return Ok(Event {
                method: method.to_owned(),
                params: message.get("params").cloned().unwrap_or(Value::Null),
                session: message
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }

    /// イベントを `timeout` まで待つ。届かなければ None（ほかの仕事をしながら待つときに使う）
    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<Event>, String> {
        if !self.pending.is_empty() {
            return self.next_event().map(Some);
        }
        self.set_read_timeout(Some(timeout));
        let read = self.socket.read();
        self.set_read_timeout(None);
        let message = match read {
            Ok(Message::Text(text)) => serde_json::from_str::<Value>(text.as_str())
                .map_err(|e| format!("DevTools の答えを読めません: {e}"))?,
            Ok(Message::Close(_)) => return Err(self.lose("ブラウザとの接続が閉じました".into())),
            Ok(_) => return Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(self.lose(format!("ブラウザとの接続が切れました: {e}"))),
        };
        self.pending.push_back(message);
        self.next_event().map(Some)
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        if let MaybeTlsStream::Plain(stream) = self.socket.get_mut() {
            let _ = stream.set_read_timeout(timeout);
        }
    }

    /// ページで式を評価し、値を返す（Promise は待つ）
    pub fn evaluate(&mut self, session: &str, expression: &str) -> Result<Value, String> {
        let result = self.call(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
            Some(session),
        )?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(format!("ページのスクリプトが失敗しました: {details}"));
        }
        Ok(result
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }
}
