use structopt::{StructOpt, clap::arg_enum};
use sc_service::{
	AbstractService, Configuration, ChainSpecExtension, RuntimeGenesis, ServiceBuilderCommand,
	config::{DatabaseConfig, KeystoreConfig}, ChainSpec, PruningMode,
};

use crate::error;
use crate::execution_strategy::*;
use crate::execution_strategy::ExecutionStrategy;

/// Parameters for block import.
#[derive(Debug, StructOpt, Clone)]
pub struct ImportParams {
	/// Specify the state pruning mode, a number of blocks to keep or 'archive'.
	///
	/// Default is to keep all block states if the node is running as a
	/// validator (i.e. 'archive'), otherwise state is only kept for the last
	/// 256 blocks.
	#[structopt(long = "pruning", value_name = "PRUNING_MODE")]
	pub pruning: Option<String>,

	/// Force start with unsafe pruning settings.
	///
	/// When running as a validator it is highly recommended to disable state
	/// pruning (i.e. 'archive') which is the default. The node will refuse to
	/// start as a validator if pruning is enabled unless this option is set.
	#[structopt(long = "unsafe-pruning")]
	pub unsafe_pruning: bool,

	/// Method for executing Wasm runtime code.
	#[structopt(
		long = "wasm-execution",
		value_name = "METHOD",
		possible_values = &WasmExecutionMethod::enabled_variants(),
		case_insensitive = true,
		default_value = "Interpreted"
	)]
	pub wasm_method: WasmExecutionMethod,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub execution_strategies: ExecutionStrategies,

	/// Limit the memory the database cache can use.
	#[structopt(long = "db-cache", value_name = "MiB", default_value = "1024")]
	pub database_cache_size: u32,

	/// Specify the state cache size.
	#[structopt(long = "state-cache-size", value_name = "Bytes", default_value = "67108864")]
	pub state_cache_size: usize,

	/// Comma separated list of targets for tracing
	#[structopt(long = "tracing-targets", value_name = "TARGETS")]
	pub tracing_targets: Option<String>,

	/// Receiver to process tracing messages
	#[structopt(
		long = "tracing-receiver",
		value_name = "RECEIVER",
		possible_values = &TracingReceiver::variants(),
		case_insensitive = true,
		default_value = "Log"
	)]
	pub tracing_receiver: TracingReceiver,
}

impl ImportParams {
	/// Put block import CLI params into `config` object.
	pub fn update_config<G, E>(
		&self,
		config: &mut Configuration<G, E>,
		role: sc_service::Roles,
		is_dev: bool,
	) -> error::Result<()>
	where
		G: RuntimeGenesis,
	{
		use sc_client_api::execution_extensions::ExecutionStrategies;

		if let Some(DatabaseConfig::Path { ref mut cache_size, .. }) = config.database {
			*cache_size = Some(self.database_cache_size);
		}

		config.state_cache_size = self.state_cache_size;

		// by default we disable pruning if the node is an authority (i.e.
		// `ArchiveAll`), otherwise we keep state for the last 256 blocks. if the
		// node is an authority and pruning is enabled explicitly, then we error
		// unless `unsafe_pruning` is set.
		config.pruning = match &self.pruning {
			Some(ref s) if s == "archive" => PruningMode::ArchiveAll,
			None if role == sc_service::Roles::AUTHORITY => PruningMode::ArchiveAll,
			None => PruningMode::default(),
			Some(s) => {
				if role == sc_service::Roles::AUTHORITY && !self.unsafe_pruning {
					return Err(error::Error::Input(
						"Validators should run with state pruning disabled (i.e. archive). \
						You can ignore this check with `--unsafe-pruning`.".to_string()
					));
				}

				PruningMode::keep_blocks(s.parse()
					.map_err(|_| error::Error::Input("Invalid pruning mode specified".to_string()))?
				)
			},
		};

		config.wasm_method = self.wasm_method.into();

		let exec = &self.execution_strategies;
		let exec_all_or = |strat: ExecutionStrategy, default: ExecutionStrategy| {
			exec.execution.unwrap_or(if strat == default && is_dev {
				ExecutionStrategy::Native
			} else {
				strat
			}).into()
		};

		config.execution_strategies = ExecutionStrategies {
			syncing: exec_all_or(exec.execution_syncing, DEFAULT_EXECUTION_SYNCING),
			importing: exec_all_or(exec.execution_import_block, DEFAULT_EXECUTION_IMPORT_BLOCK),
			block_construction:
				exec_all_or(exec.execution_block_construction, DEFAULT_EXECUTION_BLOCK_CONSTRUCTION),
			offchain_worker:
				exec_all_or(exec.execution_offchain_worker, DEFAULT_EXECUTION_OFFCHAIN_WORKER),
			other: exec_all_or(exec.execution_other, DEFAULT_EXECUTION_OTHER),
		};
		Ok(())
	}
}

arg_enum! {
	/// How to execute Wasm runtime code
	#[allow(missing_docs)]
	#[derive(Debug, Clone, Copy)]
	pub enum WasmExecutionMethod {
		// Uses an interpreter.
		Interpreted,
		// Uses a compiled runtime.
		Compiled,
	}
}

arg_enum! {
	#[allow(missing_docs)]
	#[derive(Debug, Copy, Clone, PartialEq, Eq)]
	pub enum TracingReceiver {
		Log,
		Telemetry,
		Grafana,
	}
}

impl Into<sc_tracing::TracingReceiver> for TracingReceiver {
	fn into(self) -> sc_tracing::TracingReceiver {
		match self {
			TracingReceiver::Log => sc_tracing::TracingReceiver::Log,
			TracingReceiver::Telemetry => sc_tracing::TracingReceiver::Telemetry,
			TracingReceiver::Grafana => sc_tracing::TracingReceiver::Grafana,
		}
	}
}

impl Into<sc_client_api::ExecutionStrategy> for ExecutionStrategy {
	fn into(self) -> sc_client_api::ExecutionStrategy {
		match self {
			ExecutionStrategy::Native => sc_client_api::ExecutionStrategy::NativeWhenPossible,
			ExecutionStrategy::Wasm => sc_client_api::ExecutionStrategy::AlwaysWasm,
			ExecutionStrategy::Both => sc_client_api::ExecutionStrategy::Both,
			ExecutionStrategy::NativeElseWasm => sc_client_api::ExecutionStrategy::NativeElseWasm,
		}
	}
}

impl WasmExecutionMethod {
	/// Returns list of variants that are not disabled by feature flags.
	fn enabled_variants() -> Vec<&'static str> {
		Self::variants()
			.iter()
			.cloned()
			.filter(|&name| cfg!(feature = "wasmtime") || name != "Compiled")
			.collect()
	}
}

impl Into<sc_service::config::WasmExecutionMethod> for WasmExecutionMethod {
	fn into(self) -> sc_service::config::WasmExecutionMethod {
		match self {
			WasmExecutionMethod::Interpreted => sc_service::config::WasmExecutionMethod::Interpreted,
			#[cfg(feature = "wasmtime")]
			WasmExecutionMethod::Compiled => sc_service::config::WasmExecutionMethod::Compiled,
			#[cfg(not(feature = "wasmtime"))]
			WasmExecutionMethod::Compiled => panic!(
				"Substrate must be compiled with \"wasmtime\" feature for compiled Wasm execution"
			),
		}
	}
}

/// Execution strategies parameters.
#[derive(Debug, StructOpt, Clone)]
pub struct ExecutionStrategies {
	/// The means of execution used when calling into the runtime while syncing blocks.
	#[structopt(
		long = "execution-syncing",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		default_value = DEFAULT_EXECUTION_SYNCING.as_str(),
	)]
	pub execution_syncing: ExecutionStrategy,

	/// The means of execution used when calling into the runtime while importing blocks.
	#[structopt(
		long = "execution-import-block",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		default_value = DEFAULT_EXECUTION_IMPORT_BLOCK.as_str(),
	)]
	pub execution_import_block: ExecutionStrategy,

	/// The means of execution used when calling into the runtime while constructing blocks.
	#[structopt(
		long = "execution-block-construction",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		default_value = DEFAULT_EXECUTION_BLOCK_CONSTRUCTION.as_str(),
	)]
	pub execution_block_construction: ExecutionStrategy,

	/// The means of execution used when calling into the runtime while using an off-chain worker.
	#[structopt(
		long = "execution-offchain-worker",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		default_value = DEFAULT_EXECUTION_OFFCHAIN_WORKER.as_str(),
	)]
	pub execution_offchain_worker: ExecutionStrategy,

	/// The means of execution used when calling into the runtime while not syncing, importing or constructing blocks.
	#[structopt(
		long = "execution-other",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		default_value = DEFAULT_EXECUTION_OTHER.as_str(),
	)]
	pub execution_other: ExecutionStrategy,

	/// The execution strategy that should be used by all execution contexts.
	#[structopt(
		long = "execution",
		value_name = "STRATEGY",
		possible_values = &ExecutionStrategy::variants(),
		case_insensitive = true,
		conflicts_with_all = &[
			"execution-other",
			"execution-offchain-worker",
			"execution-block-construction",
			"execution-import-block",
			"execution-syncing",
		]
	)]
	pub execution: Option<ExecutionStrategy>,
}