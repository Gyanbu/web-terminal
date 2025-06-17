use axum::{
    Router,
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures_util::{SinkExt as _, StreamExt as _};
use std::{collections::VecDeque, env, ffi::OsStr, net::SocketAddr, path::Path, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _},
    sync::{RwLock, broadcast, mpsc},
};
use tower_http::services::ServeDir;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

// Maximum number of messages to keep before removing oldest
const MAX_MESSAGES: usize = 256;

#[tokio::main]
async fn main() {
    // Initialize logging with info level as default
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Usage: cargo r -r -- <target_executable> [target_args...]");
        return;
    }
    let target_exe = Path::new(&args[1]);
    if !target_exe.exists() {
        println!();
    }
    let working_dir = target_exe.parent().unwrap();
    let target_args = &args[2..];
    // Create our program handler
    let program_handler = Arc::new(
        ProgramHandler::new(target_exe, working_dir, target_args)
            .await
            .expect("Failed to start program"),
    );

    // Build our application with shared state
    let app = Router::new()
        .fallback_service(ServeDir::new("html"))
        .route("/ws", get(ws_handler))
        .with_state(program_handler);

    // Run the server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    tracing::info!(
        "Server listening on http://{}",
        listener.local_addr().unwrap()
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

/// WebSocket handler that bridges clients to the program
async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(program_handler): State<Arc<ProgramHandler>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_connection(socket, addr, program_handler))
}

/// Handle an individual WebSocket connection
async fn handle_connection(
    socket: WebSocket,
    addr: SocketAddr,
    program_handler: Arc<ProgramHandler>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Subscribe to program output and input history
    let (mut program_rx, initial_messages) = program_handler.subscribe().await;
    let stdin_tx = program_handler.get_stdin_tx();

    // Send initial messages (both input and output history)
    for msg in initial_messages {
        if ws_sender.send(Message::Text(msg.into())).await.is_err() {
            return;
        }
    }

    // Spawn task to forward program messages to WebSocket
    let send_task = tokio::spawn(async move {
        while let Ok(msg) = program_rx.recv().await {
            if ws_sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Spawn task to forward WebSocket messages to program stdin
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(input))) = ws_receiver.next().await {
            // Broadcast the input to all clients before sending to program
            if let Err(e) = program_handler.broadcast_input(&input).await {
                tracing::error!("Failed to broadcast input: {}", e);
                break;
            }

            // Send to program stdin
            if stdin_tx.send(input.to_string()).is_err() {
                break;
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    tracing::info!("Connection closed: {}", addr);
}

/// ProgramHandler implementation with input broadcasting
struct ProgramHandler {
    program_handle: tokio::process::Child,
    stdin_tx: mpsc::UnboundedSender<String>,
    message_tx: broadcast::Sender<String>,
    message_buf: Arc<RwLock<VecDeque<String>>>,
}

impl ProgramHandler {
    async fn new<S, P>(
        program_path: S,
        working_dir_path: P,
        args: &[String],
    ) -> std::io::Result<Self>
    where
        S: AsRef<OsStr>,
        P: AsRef<Path>,
    {
        let mut program_handle = tokio::process::Command::new(program_path)
            .current_dir(working_dir_path)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;

        let message_buf = Arc::new(RwLock::new(VecDeque::with_capacity(MAX_MESSAGES)));
        let (message_tx, _) = broadcast::channel(MAX_MESSAGES);

        // Setup stdin writer
        let mut program_stdin = program_handle.stdin.take().unwrap();
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();

        tokio::spawn(async move {
            while let Some(msg) = stdin_rx.recv().await {
                if let Err(e) = async {
                    program_stdin.write_all(msg.as_bytes()).await?;
                    program_stdin.write_all(b"\n").await?;
                    program_stdin.flush().await
                }
                .await
                {
                    tracing::error!("Failed to write to stdin: {}", e);
                    break;
                }
            }
        });

        // Setup stdout reader
        let program_stdout = program_handle.stdout.take().unwrap();
        let mut program_out_reader = tokio::io::BufReader::new(program_stdout);
        let message_tx_clone2 = message_tx.clone();
        let message_buf_clone2 = Arc::clone(&message_buf);

        tokio::spawn(async move {
            let mut buf = String::new();
            loop {
                buf.clear();
                match program_out_reader.read_line(&mut buf).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = buf.trim().to_string();

                        // Broadcast and store output
                        let _ = message_tx_clone2.send(trimmed.clone());
                        let mut message_buf = message_buf_clone2.write().await;
                        if message_buf.len() >= MAX_MESSAGES {
                            message_buf.pop_front();
                        }
                        message_buf.push_back(trimmed);
                    }
                    Err(e) => {
                        tracing::error!("Read error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            program_handle,
            stdin_tx,
            message_tx,
            message_buf,
        })
    }

    /// Subscribe to both input and output messages
    async fn subscribe(&self) -> (broadcast::Receiver<String>, Vec<String>) {
        let buf = self.message_buf.read().await;
        (self.message_tx.subscribe(), buf.clone().into())
    }

    /// Broadcast input to all clients and store in history
    async fn broadcast_input(&self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        let input = input.to_string();
        // Broadcast to all clients
        self.message_tx.send(input.clone())?;

        // Store in history
        let mut message_buf = self.message_buf.write().await;
        if message_buf.len() >= MAX_MESSAGES {
            message_buf.pop_front();
        }
        message_buf.push_back(input);

        Ok(())
    }

    fn get_stdin_tx(&self) -> mpsc::UnboundedSender<String> {
        self.stdin_tx.clone()
    }
}

impl Drop for ProgramHandler {
    fn drop(&mut self) {
        if let Err(e) = self.program_handle.start_kill() {
            tracing::error!("Failed to kill child process: {}", e);
        }
    }
}
