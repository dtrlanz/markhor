use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use markhor_core::chat::{
    ChatUi, MemberId, Msg, MsgEvent, MsgId, ReasoningEvent, ToolCallEvent, ToolCallId, WorkerEvent,
    common::{ContentPart, ToolCallRequest, ToolResult},
};
use serde_json::{Value, json};
use tokio::{task::JoinSet, time::sleep};

const COMMANDS: &[&str] = &[
    "help",
    "agents",
    "tools",
    "stream",
    "reasoning",
    "seed",
    "select",
    "exit",
];

const HELP: &str = "Commands: /agents <n> | /tools <n> | /stream on|off | \
    /reasoning on|off | /seed <n> | /select | /exit";

const WORDS: &[&str] = &[
    "notes",
    "model",
    "document",
    "chunk",
    "embedding",
    "workspace",
    "member",
    "thread",
    "summary",
    "pipeline",
    "context",
    "tool",
    "agent",
    "message",
    "index",
    "folder",
    "reference",
    "draft",
    "outline",
    "source",
    "citation",
    "session",
    "prompt",
    "result",
];

const TOPICS: &[&str] = &[
    "Summarizing",
    "Searching",
    "Comparing",
    "Checking",
    "Drafting",
    "Indexing",
];

const OPTIONS: &[&str] = &[
    "Summarize the notes",
    "Search the web",
    "Draft a reply",
    "Do nothing",
];

const CITIES: &[&str] = &["Lisbon", "Tokyo", "Nairobi", "Oslo"];

const TOOL_NAMES: &[&str] = &["search_notes", "read_file", "calculator", "fetch_weather"];

#[derive(Clone, Copy)]
struct Settings {
    max_agents: usize,
    max_tools: usize,
    stream: bool,
    reasoning: bool,
}

#[derive(Default)]
struct IdGen {
    msgs: AtomicUsize,
    calls: AtomicUsize,
    workers: AtomicUsize,
}

impl IdGen {
    fn msg(&self) -> MsgId {
        MsgId(self.msgs.fetch_add(1, Ordering::Relaxed))
    }

    fn call(&self) -> ToolCallId {
        ToolCallId(self.calls.fetch_add(1, Ordering::Relaxed))
    }

    fn worker(&self) -> usize {
        self.workers.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// Drives a scripted, randomized conversation through a `ChatUi`.
pub struct Simulation {
    ui: Arc<dyn ChatUi>,
    ids: Arc<IdGen>,
    seed: u64,
    rng: fastrand::Rng,
    settings: Settings,
    user: MemberId,
    assistant: MemberId,
    platform: MemberId,
    workers: JoinSet<()>,
}

impl Simulation {
    pub fn new(ui: Arc<dyn ChatUi>, seed: u64) -> Self {
        Self {
            ui,
            ids: Arc::default(),
            seed,
            rng: fastrand::Rng::with_seed(seed),
            settings: Settings {
                max_agents: 2,
                max_tools: 3,
                stream: true,
                reasoning: true,
            },
            user: MemberId::with_display_name("user", "You", true),
            assistant: MemberId::with_display_name("assistant", "Assistant", false),
            platform: MemberId::new("platform", false),
            workers: JoinSet::new(),
        }
    }

    pub async fn run(mut self) {
        self.notice(format!(
            "Simulator started (seed {}). Type /help for commands.",
            self.seed
        ));

        loop {
            let input = self
                .ui
                .prompt_with_commands(COMMANDS.iter().map(|c| c.to_string()).collect())
                .await;
            let input = input.trim();
            if input.is_empty() {
                continue;
            }
            match input.strip_prefix('/') {
                Some(cmd) => {
                    if !self.command(cmd).await {
                        break;
                    }
                }
                None => self.user_turn(input.to_string()).await,
            }
        }

        // Let background workers finish before the process exits.
        while self.workers.join_next().await.is_some() {}
    }

    /// Runs a slash command. Returns `false` if the simulation should stop.
    async fn command(&mut self, line: &str) -> bool {
        let mut parts = line.split_whitespace();
        let name = parts.next().unwrap_or("");
        let arg = parts.next();

        let reply = match name {
            "help" => HELP.to_string(),
            "exit" | "quit" => return false,
            "agents" => match arg.and_then(|a| a.parse().ok()) {
                Some(n) => {
                    self.settings.max_agents = n;
                    format!("Background workers per turn: up to {n}.")
                }
                None => "usage: /agents <n>".to_string(),
            },
            "tools" => match arg.and_then(|a| a.parse().ok()) {
                Some(n) => {
                    self.settings.max_tools = n;
                    format!("Parallel tool calls per response: up to {n}.")
                }
                None => "usage: /tools <n>".to_string(),
            },
            "stream" => match parse_switch(arg) {
                Some(on) => {
                    self.settings.stream = on;
                    format!("Streaming {}.", on_off(on))
                }
                None => "usage: /stream on|off".to_string(),
            },
            "reasoning" => match parse_switch(arg) {
                Some(on) => {
                    self.settings.reasoning = on;
                    format!("Reasoning traces {}.", on_off(on))
                }
                None => "usage: /reasoning on|off".to_string(),
            },
            "seed" => match arg.and_then(|a| a.parse::<u64>().ok()) {
                Some(n) => {
                    self.seed = n;
                    self.rng = fastrand::Rng::with_seed(n);
                    format!("Reseeded with {n}.")
                }
                None => "usage: /seed <n>".to_string(),
            },
            "select" => {
                self.select_demo().await;
                return true;
            }
            other => format!("Unknown command: /{other}. Type /help."),
        };

        self.notice(reply);
        true
    }

    /// Emits a platform message, outside the normal agent conversation.
    fn notice(&self, text: impl Into<String>) {
        let id = self.ids.msg();
        self.ui
            .handle_message(&MsgEvent::Begin(id, self.platform.clone()));
        self.ui
            .handle_message(&MsgEvent::Full(id, Msg::Platform(text.into())));
    }

    async fn user_turn(&mut self, text: String) {
        while self.workers.try_join_next().is_some() {}

        let id = self.ids.msg();
        self.ui
            .handle_message(&MsgEvent::Begin(id, self.user.clone()));
        let lead = format!(
            "Regarding \"{}\",",
            text.chars().take(40).collect::<String>()
        );
        self.ui.handle_message(&MsgEvent::Full(
            id,
            Msg::User {
                sender: self.user.clone(),
                timestamp: Some(SystemTime::now()),
                content: vec![ContentPart::Text(text)],
            },
        ));

        for _ in 0..self.rng.usize(0..=self.settings.max_agents) {
            self.spawn_worker();
        }

        self.respond(&lead).await;

        if self.rng.usize(0..5) == 0 {
            self.select_demo().await;
        }
    }

    /// Produces an assistant reply, optionally with parallel tool calls and a
    /// follow-up message once the tools have finished.
    async fn respond(&mut self, lead: &str) {
        let sender = self.assistant.clone();
        let body = format!("{lead} {}", paragraph(&mut self.rng));

        let n_tools = self.rng.usize(0..=self.settings.max_tools);
        let requests: Vec<ToolCallRequest> = (0..n_tools).map(|_| self.tool_request()).collect();

        let msg_id = self.compose(&sender, body, requests.clone()).await;
        if requests.is_empty() {
            return;
        }

        // All calls begin before any of them completes, so their events interleave.
        let mut tools = JoinSet::new();
        for request in requests {
            let call = self.ids.call();
            self.ui
                .handle_tool_call(&ToolCallEvent::Begin(msg_id, call, request.clone()));
            let rng = self.child_rng();
            tools.spawn(run_tool(self.ui.clone(), call, request, rng));
        }
        while tools.join_next().await.is_some() {}

        let follow_up = format!("Tool results are in. {}", paragraph(&mut self.rng));
        self.compose(&sender, follow_up, vec![]).await;
    }

    /// Asks the user to pick one of a shuffled set of options via `select`.
    async fn select_demo(&mut self) {
        let sender = self.assistant.clone();
        let mut options: Vec<String> = OPTIONS.iter().map(|s| s.to_string()).collect();
        let rotate = self.rng.usize(0..options.len());
        options.rotate_left(rotate);

        self.compose(
            &sender,
            "Which option should I take next?".to_string(),
            vec![],
        )
        .await;

        let choice = self.ui.select(&options).await.and_then(|i| options.get(i));
        let reply = match choice {
            Some(option) => format!("Going with \"{option}\"."),
            None => "No option selected; I'll wait for further instructions.".to_string(),
        };
        self.compose(&sender, reply, vec![]).await;
    }

    /// Emits a complete agent message: begin, optional reasoning, streamed
    /// parts, and the full message. Returns the message id.
    async fn compose(
        &mut self,
        sender: &MemberId,
        body: String,
        tool_calls: Vec<ToolCallRequest>,
    ) -> MsgId {
        let id = self.ids.msg();
        self.ui.handle_message(&MsgEvent::Begin(id, sender.clone()));

        if self.settings.reasoning {
            self.reason(id).await;
        }

        if self.settings.stream {
            for word in body.split_inclusive(' ') {
                sleep(ms(&mut self.rng, 20, 90)).await;
                self.ui
                    .handle_message(&MsgEvent::Part(id, ContentPart::Text(word.to_string())));
            }
        }

        if self.rng.usize(0..8) == 0 {
            self.ui.handle_message(&MsgEvent::Log(
                id,
                "generated with simulated latency".to_string(),
            ));
        }

        self.ui.handle_message(&MsgEvent::Full(
            id,
            Msg::Agent {
                sender: sender.clone(),
                timestamp: Some(SystemTime::now()),
                content: vec![ContentPart::Text(body)],
                tool_calls,
            },
        ));
        id
    }

    async fn reason(&mut self, id: MsgId) {
        self.ui.handle_reasoning(&ReasoningEvent::Begin(id));

        let mut trace = String::new();
        for _ in 0..self.rng.usize(1..=2) {
            sleep(ms(&mut self.rng, 30, 120)).await;
            let title = pick(&mut self.rng, TOPICS).to_string();
            let prose = sentence(&mut self.rng);
            if self.settings.stream {
                self.ui
                    .handle_reasoning(&ReasoningEvent::Part(id, format!("{prose} ")));
            }
            self.ui
                .handle_reasoning(&ReasoningEvent::Summary(id, title, prose.clone()));
            trace.push_str(&prose);
            trace.push(' ');
        }

        if self.rng.usize(0..4) == 0 {
            self.ui
                .handle_reasoning(&ReasoningEvent::Log(id, "trace condensed".to_string()));
        }
        self.ui.handle_reasoning(&ReasoningEvent::Full(id, trace));
    }

    fn tool_request(&mut self) -> ToolCallRequest {
        let call = self.ids.call();
        let name = pick(&mut self.rng, TOOL_NAMES);
        let arguments = match name {
            "search_notes" => json!({ "query": pick(&mut self.rng, WORDS) }),
            "read_file" => json!({ "path": format!("notes/{}.md", pick(&mut self.rng, WORDS)) }),
            "calculator" => json!({
                "expr": format!("{} * {}", self.rng.u32(1..100), self.rng.u32(1..100))
            }),
            _ => json!({ "city": pick(&mut self.rng, CITIES) }),
        };
        ToolCallRequest {
            id: format!("req-{}", *call),
            name: name.to_string(),
            arguments,
        }
    }

    fn spawn_worker(&mut self) {
        let n = self.ids.worker();
        let member =
            MemberId::with_display_name(&format!("worker-{n}"), &format!("Worker {n}"), false);
        let rng = self.child_rng();
        self.workers.spawn(run_worker(self.ui.clone(), member, rng));
    }

    /// Creates an independent RNG for a spawned task, seeded from the main one.
    fn child_rng(&mut self) -> fastrand::Rng {
        fastrand::Rng::with_seed(self.rng.u64(..))
    }
}

async fn run_worker(ui: Arc<dyn ChatUi>, member: MemberId, mut rng: fastrand::Rng) {
    ui.handle_worker(&WorkerEvent::Begin(member.clone()));

    for _ in 0..rng.usize(1..=3) {
        sleep(ms(&mut rng, 100, 500)).await;
        let title = pick(&mut rng, TOPICS).to_string();
        ui.handle_worker(&WorkerEvent::Summary(
            member.clone(),
            title,
            sentence(&mut rng),
        ));
    }

    sleep(ms(&mut rng, 100, 500)).await;
    // Workers sometimes finish without a text representation.
    let text = if rng.usize(0..4) == 0 {
        String::new()
    } else {
        sentence(&mut rng)
    };
    ui.handle_worker(&WorkerEvent::Full(member, text));
}

async fn run_tool(
    ui: Arc<dyn ChatUi>,
    call: ToolCallId,
    request: ToolCallRequest,
    mut rng: fastrand::Rng,
) {
    for _ in 0..rng.usize(0..=3) {
        sleep(ms(&mut rng, 40, 300)).await;
        ui.handle_tool_call(&ToolCallEvent::Part(
            call,
            format!("{}\n", sentence(&mut rng)),
        ));
    }

    if rng.usize(0..4) == 0 {
        ui.handle_tool_call(&ToolCallEvent::Log(
            call,
            "retrying after a transient error".to_string(),
        ));
        sleep(ms(&mut rng, 100, 300)).await;
    }

    sleep(ms(&mut rng, 60, 400)).await;
    let content = fake_result(&request, &mut rng);
    ui.handle_tool_call(&ToolCallEvent::Full(
        call,
        ToolResult {
            call_id: request.id,
            name: request.name,
            content,
        },
    ));
}

fn fake_result(request: &ToolCallRequest, rng: &mut fastrand::Rng) -> Value {
    if rng.usize(0..6) == 0 {
        return json!({ "error": "simulated failure" });
    }
    match request.name.as_str() {
        "search_notes" => json!({
            "query": request.arguments["query"],
            "matches": rng.usize(0..9),
        }),
        "read_file" => json!({
            "path": request.arguments["path"],
            "bytes": rng.usize(100..5000),
        }),
        "calculator" => json!({
            "expr": request.arguments["expr"],
            "value": rng.u32(1..10000),
        }),
        _ => json!({
            "city": request.arguments["city"],
            "forecast": pick(rng, &["sunny", "rain", "snow", "fog"]),
        }),
    }
}

fn pick<'a>(rng: &mut fastrand::Rng, items: &[&'a str]) -> &'a str {
    items[rng.usize(0..items.len())]
}

fn ms(rng: &mut fastrand::Rng, lo: u64, hi: u64) -> Duration {
    Duration::from_millis(rng.u64(lo..=hi))
}

fn sentence(rng: &mut fastrand::Rng) -> String {
    let n = rng.usize(5..=11);
    let mut s = (0..n)
        .map(|_| pick(rng, WORDS))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s.push('.');
    s
}

fn paragraph(rng: &mut fastrand::Rng) -> String {
    let n = rng.usize(1..=3);
    (0..n).map(|_| sentence(rng)).collect::<Vec<_>>().join(" ")
}

fn parse_switch(arg: Option<&str>) -> Option<bool> {
    match arg? {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}
