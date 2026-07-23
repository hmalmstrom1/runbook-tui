# General Rust Project Architecture & Best Practices

Guidelines for structuring production-ready Rust projects, implementing extendable plugin systems, and applying idiomatic language patterns.

## 1. File Structure Layout

### The Main-Lib Split
* **Slim Binary Target**: Keep `src/main.rs` under 50 lines. It should only manage CLI flag parsing, telemetry initialization, and runtime startup.
* **Core Library Target**: Move all business domain logic into a decoupled library crate via `src/lib.rs` to allow code caching and simplify integration testing.

### Cargo Workspaces
Scale large codebases by breaking features into discrete internal crates managed by a top-level workspace.

```text
my-project/
├── Cargo.toml            # Root workspace configuration
├── src/
│   └── main.rs           # Slim entry point binary
└── crates/
    ├── app_core/         # Pure domain logic, state models, and engines
    │   ├── Cargo.toml
    │   └── src/lib.rs
    ├── app_tui/          # UI layout rendering engine (e.g., Ratatui layer)
    │   ├── Cargo.toml
    │   └── src/lib.rs
    └── app_plugin_api/   # Plugin interfaces and engine runtime definitions
        ├── Cargo.toml
        └── src/lib.rs
```

### Domain-Driven Module Isolation
Organize file trees by feature boundaries rather than technical roles. Avoid monolithic files like `models.rs` or `types.rs`.

```text
src/
├── lib.rs
├── billing/             # All payment processing domain code
│   ├── mod.rs
│   ├── ledger.rs
│   └── stripe_client.rs
└── storage/             # All persistence layer domain code
    ├── mod.rs
    └── cache.rs
```

---

## 2. Plugin & Extensibility Architectures

### Wasm Sandbox (Highly Recommended)
* **Strategy**: Compile downstream plugins into standalone WebAssembly binaries (`.wasm`) and execute them via native runtimes like `wasmtime` or abstractions like `Extism`.
* **Benefits**: Language agnostic (plugins can be written in Rust, Go, or TypeScript), strictly sandboxed memory, and completely crash-safe for the host process.

### Embedded Scripting (High Iteration Speed)
* **Lua (`mlua`)**: The industry standard configuration and scripting engine for high-performance command line software (e.g., Neovim).
* **Rhai**: A lightweight, native Rust-embedded scripting language featuring a clean syntax heavily inspired by standard Rust constructs.

### Trait-Driven Architecture
Define explicit interface contracts using Send/Sync bounded public traits to securely route execution state.

```rust
pub trait AppPlugin: std::fmt::Debug + Send + Sync {
    /// Executed immediately on application setup
    fn on_init(&self, context: &mut AppContext);
    
    /// Executed whenever a systemic event gets dispatched
    fn on_event(&self, event: &Event, context: &mut AppContext);
}
```

---

## 3. General Rust Idioms & Best Practices

### Structured Error Handling
* **Libraries (`crates/*`)**: Implement `thiserror` to model structured, strongly-typed error enumerations with explicit `Display` trait overrides.
* **Binaries (`src/main.rs`)**: Leverage `eyre` or `anyhow` to aggregate errors, attach system strings, and capture contextual backtraces.

### Efficient Memory Allocations
* **Mitigate Blind Clones**: Do not sprinkle `.clone()` calls to bypass compiler checks. Rely on borrow lifetimes (`&T`) or employ atomic reference-counting pointers (`Arc<T>`) for shared cross-thread data layouts.
* **The Newtype Pattern**: Prevent semantic type mixing bugs by wrapping primitive types into unique single-field domain objects.

```rust
// Strongly distinct types; compiler prevents accidental transposition
pub struct UserId(pub u64);
pub struct TeamId(pub u64);
```

### Advanced Compiler Optimization
* **Typestate Verification**: Use the type system to guarantee code operations are structurally valid at compile time.

```rust
// An object cannot invoke execution routines until it reaches the correct phase
pub struct JobBuilder<State> { _marker: std::marker::PhantomData<State> }
pub struct Ready;
pub struct Unconfigured;

impl JobBuilder<Ready> {
    pub fn execute(self) { /* Valid operation */ }
}
```
* **Static Dispatch Preference**: Default to utilizing generics for trait parameters (`fn process<T: Trait>(item: T)`) to unlock compiler monomorphization and inlining over heavy dynamic dispatch tables (`&dyn Trait`).
