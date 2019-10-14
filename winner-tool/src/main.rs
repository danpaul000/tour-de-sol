//! This tool calculates the quantitative category winners for Tour de SOL.
//!
//! NOTE: Ledger processing uses native programs, so this tool must be invoked with `cargo run`.
//! If installed with `cargo install` the native programs may not be linked properly.

mod availability;
mod confirmation_latency;
mod rewards_earned;
mod utils;
mod winner;

use clap::{crate_description, crate_name, crate_version, value_t, value_t_or_exit, App, Arg};
use confirmation_latency::{SlotVoterSegments, VoterRecord};
use solana_cli::input_validators::is_pubkey;
use solana_core::blocktree::Blocktree;
use solana_core::blocktree_processor::{process_blocktree, ProcessOptions};
use solana_runtime::bank::Bank;
use solana_sdk::genesis_block::GenesisBlock;
use solana_sdk::native_token::sol_to_lamports;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

fn main() {
    solana_logger::setup();

    let matches = App::new(crate_name!())
        .about(crate_description!())
        .version(crate_version!())
        .arg(
            Arg::with_name("ledger")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .help("Use directory for ledger location"),
        )
        .arg(
            Arg::with_name("starting_balance")
                .long("starting-balance")
                .value_name("SOL")
                .takes_value(true)
                .default_value("1000")
                .help("Starting balance of validators at the beginning of TdS"),
        )
        .arg(
            Arg::with_name("baseline_validator")
                .long("baseline-validator")
                .value_name("PUBKEY")
                .takes_value(true)
                .required(true)
                .validator(is_pubkey)
                .help("Public key of the baseline validator"),
        )
        .arg(
            Arg::with_name("bootstrap_leader")
                .long("bootstrap-leader")
                .value_name("PUBKEY")
                .takes_value(true)
                .required(true)
                .validator(is_pubkey)
                .help("Public key of the bootstrap leader"),
        )
        .arg(
            Arg::with_name("final_slot")
                .long("final-slot")
                .value_name("SLOT")
                .takes_value(true)
                .help("Final slot of TdS ledger"),
        )
        .get_matches();

    let ledger_path = PathBuf::from(value_t_or_exit!(matches, "ledger", String));
    let starting_balance_sol = value_t_or_exit!(matches, "starting_balance", f64);
    let baseline_id_string = value_t_or_exit!(matches, "baseline_validator", String);
    let bootstrap_id_string = value_t_or_exit!(matches, "bootstrap_leader", String);
    let final_slot = value_t!(matches, "final_slot", u64).ok();

    let baseline_id = Pubkey::from_str(&baseline_id_string).unwrap_or_else(|err| {
        eprintln!(
            "Failed to create a valid pubkey from {}: {}",
            baseline_id_string, err
        );
        exit(1);
    });

    let bootstrap_id = Pubkey::from_str(&bootstrap_id_string).unwrap_or_else(|err| {
        eprintln!(
            "Failed to create a valid pubkey from {}: {}",
            bootstrap_id_string, err
        );
        exit(1);
    });

    let genesis_block = GenesisBlock::load(&ledger_path).unwrap_or_else(|err| {
        eprintln!(
            "Failed to open ledger genesis_block at {:?}: {}",
            ledger_path, err
        );
        exit(1);
    });

    let blocktree = match Blocktree::open(&ledger_path) {
        Ok(blocktree) => blocktree,
        Err(err) => {
            eprintln!("Failed to open ledger at {:?}: {}", ledger_path, err);
            exit(1);
        }
    };

    // Track voter record after each entry
    let voter_record: Arc<RwLock<VoterRecord>> = Arc::default();
    let slot_voter_segments: Arc<RwLock<SlotVoterSegments>> = Arc::default();
    let entry_callback = {
        let voter_record = voter_record.clone();
        let slot_voter_segments = slot_voter_segments.clone();
        Arc::new(move |bank: &Bank| {
            confirmation_latency::on_entry(
                bank.slot(),
                bank.vote_accounts(),
                &mut voter_record.write().unwrap(),
                &mut slot_voter_segments.write().unwrap(),
            );
        })
    };

    let opts = ProcessOptions {
        verify_ledger: false,
        dev_halt_at_slot: final_slot,
        full_leader_cache: true,
        entry_callback: Some(entry_callback),
        override_num_threads: Some(1),
    };

    let excluded_set = {
        let mut set = HashSet::new();
        set.insert(baseline_id);
        set.insert(bootstrap_id);
        set
    };

    println!("Processing ledger...");
    match process_blocktree(&genesis_block, &blocktree, None, opts) {
        Ok((bank_forks, _bank_forks_info, leader_schedule_cache)) => {
            let bank = bank_forks.working_bank();
            let starting_balance = sol_to_lamports(starting_balance_sol);

            let rewards_earned_winners =
                rewards_earned::compute_winners(&bank, &excluded_set, starting_balance);
            println!("{:#?}", rewards_earned_winners);

            let availability_winners = availability::compute_winners(
                &bank,
                &blocktree,
                &baseline_id,
                &excluded_set,
                &leader_schedule_cache,
            );
            println!("{:#?}", availability_winners);

            let latency_winners = confirmation_latency::compute_winners(
                &bank,
                &baseline_id,
                &excluded_set,
                &mut voter_record.write().unwrap(),
                &mut slot_voter_segments.write().unwrap(),
            );
            println!("{:#?}", latency_winners);
        }
        Err(err) => {
            eprintln!("Failed to process ledger: {:?}", err);
            exit(1);
        }
    }
}