// Copyright (c) 2019 Alain Brenzikofer
// This file is part of Encointer
//
// Encointer is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Encointer is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Encointer.  If not, see <http://www.gnu.org/licenses/>.

//! # Encointer Parachain Runtime
//!
//! Encointer runtime containing all the specific logic to:
//!  * perform ceremonies and receive a community income
//!  * pay fees in the respective community currency
//!
//! The configuration (especially XCM) is almost identical to `statemine`. Therefore, upstream
//! updates should always check the diff to see if there are some configuration updates.

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime!` does a lot of recursion and requires us to increase the limit to 256.
#![recursion_limit = "256"]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

mod weights;
pub mod xcm_config;

use cumulus_pallet_parachain_system::RelayNumberStrictlyIncreases;
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_runtime::{
	create_runtime_str, generic, impl_opaque_keys,
	traits::{AccountIdLookup, BlakeTwo256, Block as BlockT, Verify},
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult, Perbill, Permill,
};

use sp_std::prelude::*;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

use codec::{Decode, Encode, MaxEncodedLen};
use encointer_balances_tx_payment::{AssetBalanceOf, AssetIdOf, BalanceToCommunityBalance};
use frame_support::{
	construct_runtime,
	dispatch::DispatchClass,
	parameter_types,
	traits::{
		tokens::ConversionToAssetBalance, ConstBool, Contains, EitherOfDiverse, EqualPrivilegeOnly,
		InstanceFilter,
	},
	weights::{ConstantMultiplier, Weight},
	PalletId, RuntimeDebug,
};
};
use frame_system::{
	limits::{BlockLength, BlockWeights},
	EnsureRoot,
};
pub use parachains_common as common;
pub use parachains_common::MILLISECS_PER_BLOCK;

use parachains_common::{
	opaque, AuraId, AVERAGE_ON_INITIALIZE_RATIO, DAYS, HOURS, MAXIMUM_BLOCK_WEIGHT,
	NORMAL_DISPATCH_RATIO, SLOT_DURATION,
};
use xcm_config::{KsmLocation, XcmConfig, XcmOriginToTransactDispatchOrigin};

#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

// A few exports that help ease life for downstream crates.
pub use pallet_encointer_balances::Call as EncointerBalancesCall;
pub use pallet_encointer_bazaar::Call as EncointerBazaarCall;
pub use pallet_encointer_ceremonies::Call as EncointerCeremoniesCall;
pub use pallet_encointer_communities::Call as EncointerCommunitiesCall;
// pub use pallet_encointer_personhood_oracle::Call as EncointerPersonhoodOracleCall;
pub use pallet_encointer_faucet::Call as EncointerFaucetCall;
pub use pallet_encointer_reputation_commitments::Call as EncointerReputationCommitmentsCall;
pub use pallet_encointer_scheduler::Call as EncointerSchedulerCall;
// pub use pallet_encointer_sybil_gate_template::Call as EncointerSybilGateCall;

pub use encointer_primitives::{
	balances::{BalanceEntry, BalanceType, Demurrage},
	bazaar::{BusinessData, BusinessIdentifier, OfferingData},
	ceremonies::{AggregatedAccountData, CeremonyIndexType, CeremonyInfo, CommunityReputation},
	common::PalletString,
	communities::{CommunityIdentifier, Location},
	scheduler::CeremonyPhaseType,
};
use sp_core::ConstU32;

// XCM imports
// Polkadot imports
use pallet_xcm::{EnsureXcm, IsMajorityOfBody};
use polkadot_runtime_common::{BlockHashCount, SlowAdjustingFeeUpdate};
use xcm::latest::BodyId;
use xcm_executor::XcmExecutor;

// Added by encointer
pub(crate) use runtime_common::{
	currency::*,
	deal_with_fees::FeesToTreasury,
	fee::WeightToFee,
	weights::{BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight},
};

/// A type to hold UTC unix epoch [ms]
pub type Moment = u64;
pub const ONE_DAY: Moment = 86_400_000;

pub type AssetId = AssetIdOf<Runtime>;
pub type AssetBalance = AssetBalanceOf<Runtime>;

impl_opaque_keys! {
	pub struct SessionKeys {
		pub aura: Aura,
	}
}

/// This runtime version.
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: create_runtime_str!("encointer-parachain"),
	impl_name: create_runtime_str!("encointer-parachain"),
	authoring_version: 1,
	spec_version: 16,
	impl_version: 1,
	apis: RUNTIME_API_VERSIONS,
	transaction_version: 3,
	state_version: 0,
};

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
	NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

//TODO add meaningful values
parameter_types! {
	pub const ProxyDepositBase: Balance = 32;
	pub const ProxyDepositFactor: Balance = 32;
	pub const MaxProxies: u16 = 32;
	pub const AnnouncementDepositBase: Balance = 32;
	pub const AnnouncementDepositFactor: Balance = 32;
	pub const MaxPending: u16 = 32;
}

/// The type used to represent the kinds of proxying allowed.
#[derive(
	Copy,
	Clone,
	Eq,
	PartialEq,
	Ord,
	PartialOrd,
	Encode,
	Decode,
	RuntimeDebug,
	scale_info::TypeInfo,
	MaxEncodedLen,
)]
pub enum ProxyType {
	Any,
	NonTransfer,
	BazaarEdit,
}

impl Default for ProxyType {
	fn default() -> Self {
		Self::Any
	}
}
impl InstanceFilter<RuntimeCall> for ProxyType {
	fn filter(&self, c: &RuntimeCall) -> bool {
		match self {
			ProxyType::Any => true,
			ProxyType::NonTransfer => matches!(c, RuntimeCall::EncointerBazaar(..)),
			ProxyType::BazaarEdit => matches!(
				c,
				RuntimeCall::EncointerBazaar(EncointerBazaarCall::create_offering { .. }) |
					RuntimeCall::EncointerBazaar(EncointerBazaarCall::update_offering { .. }) |
					RuntimeCall::EncointerBazaar(EncointerBazaarCall::delete_offering { .. })
			),
		}
	}

	fn is_superset(&self, o: &Self) -> bool {
		match (self, o) {
			(x, y) if x == y => true,
			(ProxyType::Any, _) => true,
			(_, ProxyType::Any) => false,
			(ProxyType::NonTransfer, _) => true,
			_ => false,
		}
	}
}

impl pallet_proxy::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type Currency = Balances;
	type ProxyType = ProxyType;
	type ProxyDepositBase = ProxyDepositBase;
	type ProxyDepositFactor = ProxyDepositFactor;
	type MaxProxies = MaxProxies;
	type WeightInfo = weights::pallet_proxy::WeightInfo<Runtime>;
	type MaxPending = MaxPending;
	type CallHasher = BlakeTwo256;
	type AnnouncementDepositBase = AnnouncementDepositBase;
	type AnnouncementDepositFactor = AnnouncementDepositFactor;
}

parameter_types! {
	pub const Version: RuntimeVersion = VERSION;
	pub RuntimeBlockLength: BlockLength =
		BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
		.base_block(BlockExecutionWeight::get())
		.for_class(DispatchClass::all(), |weights| {
			weights.base_extrinsic = ExtrinsicBaseWeight::get();
		})
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
			// Operational transactions have some extra reserved space, so that they
			// are included even if block reached `MAXIMUM_BLOCK_WEIGHT`.
			weights.reserved = Some(
				MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT
			);
		})
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic();
	pub const SS58Prefix: u8 = 2;
}

pub struct BaseFilter;
impl Contains<RuntimeCall> for BaseFilter {
	fn contains(_c: &RuntimeCall) -> bool {
		true
	}
}

// Configure FRAME pallets to include in runtime.
impl frame_system::Config for Runtime {
	type BaseCallFilter = BaseFilter;
	// The block type.
	type Block = generic::Block<Header, UncheckedExtrinsic>;
	type BlockWeights = RuntimeBlockWeights;
	type BlockLength = RuntimeBlockLength;
	type AccountId = AccountId;
	type RuntimeCall = RuntimeCall;
	type Lookup = AccountIdLookup<AccountId, ()>;
	type Nonce = Index;
	type Hash = Hash;
	type Hashing = BlakeTwo256;
	type RuntimeEvent = RuntimeEvent;
	type RuntimeOrigin = RuntimeOrigin;
	type BlockHashCount = BlockHashCount;
	type DbWeight = RocksDbWeight;
	type Version = Version;
	type PalletInfo = PalletInfo;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type AccountData = pallet_balances::AccountData<Balance>;
	type SystemWeightInfo = weights::frame_system::WeightInfo<Runtime>;
	type SS58Prefix = SS58Prefix;
	type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
	type MaxConsumers = frame_support::traits::ConstU32<16>;
	type AllowMultipleBlocksPerSlot = ConstBool<false>;
}

parameter_types! {
	pub const MinimumPeriod: u64 = SLOT_DURATION / 2;
}

impl pallet_timestamp::Config for Runtime {
	/// A timestamp: milliseconds since the unix epoch.
	type Moment = Moment;
	type OnTimestampSet = EncointerScheduler;
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = weights::pallet_timestamp::WeightInfo<Runtime>;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = EXISTENTIAL_DEPOSIT;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = MaxLocks;
	/// The type for recording an account's balance.
	type Balance = Balance;
	/// The ubiquitous event type.
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = weights::pallet_balances::WeightInfo<Runtime>;
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = ();
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<0>;
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	/// Relay Chain `TransactionByteFee` / 10, same as statemine
	pub const TransactionByteFee: Balance = MILLICENTS;
	pub const OperationalFeeMultiplier: u8 = 5;
}

impl pallet_transaction_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	// `FeesToTreasury is an encointer adaptation.
	type OnChargeTransaction =
		pallet_transaction_payment::CurrencyAdapter<Balances, FeesToTreasury<Runtime>>;
	type WeightToFee = WeightToFee;
	type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
	type FeeMultiplierUpdate = SlowAdjustingFeeUpdate<Self>;
	type OperationalFeeMultiplier = OperationalFeeMultiplier;
}

parameter_types! {
	pub const ProposalBond: Permill = Permill::from_percent(5);
	pub const ProposalBondMinimum: Balance = 100 * MILLICENTS;
	pub const ProposalBondMaximum: Balance = 500 * CENTS;
	pub const SpendPeriod: BlockNumber = 6 * DAYS;
	pub const Burn: Permill = Permill::from_percent(1);
	pub const TreasuryPalletId: PalletId = PalletId(*b"py/trsry");
	pub const MaxApprovals: u32 = 10;
}

impl pallet_treasury::Config for Runtime {
	type PalletId = TreasuryPalletId;
	type Currency = pallet_balances::Pallet<Runtime>;
	type ApproveOrigin = MoreThanHalfCouncil;
	type RejectOrigin = MoreThanHalfCouncil;
	type RuntimeEvent = RuntimeEvent;
	type OnSlash = (); //No proposal
	type ProposalBond = ProposalBond;
	type ProposalBondMinimum = ProposalBondMinimum;
	type ProposalBondMaximum = ProposalBondMaximum;
	type SpendPeriod = SpendPeriod; //Cannot be 0: Error: Thread 'tokio-runtime-worker' panicked at 'attempt to calculate the remainder with a divisor of zero
	type Burn = (); //No burn
	type BurnDestination = (); //No burn
	type SpendFunds = (); //No spend, no bounty
	type MaxApprovals = MaxApprovals;
	type WeightInfo = weights::pallet_treasury::WeightInfo<Runtime>;
	type SpendOrigin = frame_support::traits::NeverEnsureOrigin<Balance>; //No spend, no bounty
}

impl pallet_utility::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeCall = RuntimeCall;
	type PalletsOrigin = OriginCaller;
	type WeightInfo = weights::pallet_utility::WeightInfo<Runtime>;
}

parameter_types! {
	pub MaximumSchedulerWeight: Weight = Perbill::from_percent(80) *
		RuntimeBlockWeights::get().max_block;
	pub const MaxScheduledPerBlock: u32 = 50;
}

impl pallet_scheduler::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type RuntimeOrigin = RuntimeOrigin;
	type PalletsOrigin = OriginCaller;
	type RuntimeCall = RuntimeCall;
	type MaximumWeight = MaximumSchedulerWeight;
	type ScheduleOrigin = MoreThanHalfCouncil;
	type MaxScheduledPerBlock = MaxScheduledPerBlock;
	type WeightInfo = pallet_scheduler::weights::SubstrateWeight<Runtime>;
	type OriginPrivilegeCmp = EqualPrivilegeOnly;
	type Preimages = ();
}

parameter_types! {
	pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
	pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
}

impl cumulus_pallet_parachain_system::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnSystemEvent = ();
	type SelfParaId = parachain_info::Pallet<Runtime>;
	type DmpMessageHandler = DmpQueue;
	type ReservedDmpWeight = ReservedDmpWeight;
	type OutboundXcmpMessageSource = XcmpQueue;
	type XcmpMessageHandler = XcmpQueue;
	type ReservedXcmpWeight = ReservedXcmpWeight;
	type CheckAssociatedRelayNumber = RelayNumberStrictlyIncreases;
}

// Added by encointer
impl pallet_insecure_randomness_collective_flip::Config for Runtime {}

impl parachain_info::Config for Runtime {}

impl cumulus_pallet_aura_ext::Config for Runtime {}

parameter_types! {
	pub const ExecutiveBody: BodyId = BodyId::Executive;
}

impl cumulus_pallet_xcmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type ChannelInfo = ParachainSystem;
	type VersionWrapper = PolkadotXcm;
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
	type ControllerOrigin = EitherOfDiverse<
		EnsureRoot<AccountId>,
		EnsureXcm<IsMajorityOfBody<KsmLocation, ExecutiveBody>>,
	>;
	type ControllerOriginConverter = XcmOriginToTransactDispatchOrigin;
	type WeightInfo = weights::cumulus_pallet_xcmp_queue::WeightInfo<Runtime>;
	type PriceForSiblingDelivery = ();
}

impl cumulus_pallet_dmp_queue::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type ExecuteOverweightOrigin = EnsureRoot<AccountId>;
}

parameter_types! {
	pub const Period: u32 = 6 * HOURS;
	pub const Offset: u32 = 0;
	pub const MaxAuthorities: u32 = 100_000;
}

parameter_types! {
	pub const MomentsPerDay: Moment = 86_400_000; // [ms/d]
	pub const DefaultDemurrage: Demurrage = Demurrage::from_bits(0x0000000000000000000001E3F0A8A973_i128);
	pub const EncointerExistentialDeposit: BalanceType = BalanceType::from_bits(0x0000000000000000000053e2d6238da4_i128);
	pub const MeetupSizeTarget: u64 = 10;
	pub const MeetupMinSize: u64 = 3;
	pub const MeetupNewbieLimitDivider: u64 = 2; // 2 means 1/3 of participants may be newbies
	pub const FaucetPalletId: PalletId = PalletId(*b"ectrfct0");
}

impl pallet_encointer_scheduler::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type OnCeremonyPhaseChange = EncointerCeremonies;
	type MomentsPerDay = MomentsPerDay;
	type CeremonyMaster = MoreThanHalfCouncil;
	type WeightInfo = weights::pallet_encointer_scheduler::WeightInfo<Runtime>;
}

impl pallet_encointer_ceremonies::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Public = <Signature as Verify>::Signer;
	type Signature = Signature;
	type RandomnessSource = RandomnessCollectiveFlip;
	type MeetupSizeTarget = MeetupSizeTarget;
	type MeetupMinSize = MeetupMinSize;
	type MeetupNewbieLimitDivider = MeetupNewbieLimitDivider;
	type CeremonyMaster = MoreThanHalfCouncil;
	type WeightInfo = weights::pallet_encointer_ceremonies::WeightInfo<Runtime>;
	type MaxAttestations = ConstU32<100>;
}

impl pallet_encointer_communities::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type CommunityMaster = MoreThanHalfCouncil;
	type TrustableForNonDestructiveAction = MoreThanHalfCouncil;
	type WeightInfo = weights::pallet_encointer_communities::WeightInfo<Runtime>;
	type MaxCommunityIdentifiers = ConstU32<10000>;
	type MaxBootstrappers = ConstU32<10000>;
	type MaxLocationsPerGeohash = ConstU32<10000>;
	type MaxCommunityIdentifiersPerGeohash = ConstU32<10000>;
}

impl pallet_encointer_balances::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type DefaultDemurrage = DefaultDemurrage;
	type ExistentialDeposit = EncointerExistentialDeposit;
	type CeremonyMaster = MoreThanHalfCouncil;
	type WeightInfo = weights::pallet_encointer_balances::WeightInfo<Runtime>;
}

impl pallet_encointer_bazaar::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_encointer_bazaar::WeightInfo<Runtime>;
}

impl pallet_encointer_reputation_commitments::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::pallet_encointer_reputation_commitments::WeightInfo<Runtime>;
}

impl pallet_encointer_faucet::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type ControllerOrigin = EnsureRoot<AccountId>;
	type Currency = Balances;
	type PalletId = FaucetPalletId;
	type WeightInfo = weights::pallet_encointer_faucet::WeightInfo<Runtime>;
}

impl pallet_aura::Config for Runtime {
	type AuthorityId = AuraId;
	type DisabledValidators = ();
	type MaxAuthorities = MaxAuthorities;
}

parameter_types! {
	pub const CouncilMotionDuration: BlockNumber = 7 * DAYS;
	pub const CouncilMaxProposals: u32 = 100;
	pub const CouncilMaxMembers: u32 = 100;
	pub MaxProposalWeight: Weight = sp_runtime::Perbill::from_percent(50) * RuntimeBlockWeights::get().max_block;
}

type MoreThanHalfCouncil = EitherOfDiverse<
	EnsureRoot<AccountId>,
	pallet_collective::EnsureProportionMoreThan<AccountId, CouncilCollective, 1, 2>,
>;

pub type CouncilCollective = pallet_collective::Instance1;
impl pallet_collective::Config<CouncilCollective> for Runtime {
	type RuntimeOrigin = RuntimeOrigin;
	type Proposal = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type MotionDuration = CouncilMotionDuration;
	type MaxProposals = CouncilMaxProposals;
	type DefaultVote = pallet_collective::PrimeDefaultVote;
	type MaxMembers = CouncilMaxMembers;
	type WeightInfo = weights::pallet_collective::WeightInfo<Runtime>;
	type SetMembersOrigin = MoreThanHalfCouncil;
	type MaxProposalWeight = MaxProposalWeight;
}

// support for collective pallet
impl pallet_membership::Config<pallet_membership::Instance1> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type AddOrigin = MoreThanHalfCouncil;
	type RemoveOrigin = MoreThanHalfCouncil;
	type SwapOrigin = MoreThanHalfCouncil;
	type ResetOrigin = MoreThanHalfCouncil;
	type PrimeOrigin = MoreThanHalfCouncil;
	type MembershipInitialized = Collective;
	type MembershipChanged = Collective;
	type MaxMembers = CouncilMaxMembers;
	type WeightInfo = weights::pallet_membership::WeightInfo<Runtime>;
}

// Allow fee payment in community currency
impl pallet_asset_tx_payment::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Fungibles = pallet_encointer_balances::Pallet<Runtime>;
	type OnChargeAssetTransaction = pallet_asset_tx_payment::FungiblesAdapter<
		encointer_balances_tx_payment::BalanceToCommunityBalance<Runtime>,
		encointer_balances_tx_payment::BurnCredit,
	>;
}

construct_runtime! {
	pub enum Runtime {
		// System support stuff.
		System: frame_system::{Pallet, Call, Config<T>, Storage, Event<T>} = 0,
		ParachainSystem: cumulus_pallet_parachain_system::{
			Pallet, Call, Config, Storage, Inherent, Event<T>, ValidateUnsigned,
		} = 1,
		RandomnessCollectiveFlip: pallet_insecure_randomness_collective_flip::{Pallet, Storage} = 2,
		Timestamp: pallet_timestamp::{Pallet, Call, Storage, Inherent} = 3,
		ParachainInfo: parachain_info::{Pallet, Storage, Config} = 4,

		// Monetary stuff.
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>} = 10,
		TransactionPayment: pallet_transaction_payment::{Pallet, Storage, Event<T>} = 11,
		AssetTxPayment: pallet_asset_tx_payment::{Pallet, Storage, Event<T>} = 12,

		Aura: pallet_aura::{Pallet, Storage, Config<T>} = 23,
		AuraExt: cumulus_pallet_aura_ext::{Pallet, Storage, Config} = 24,

		// XCM helpers.
		XcmpQueue: cumulus_pallet_xcmp_queue::{Pallet, Call, Storage, Event<T>} = 30,
		PolkadotXcm: pallet_xcm::{Pallet, Call, Storage, Event<T>, Origin, Config} = 31,
		CumulusXcm: cumulus_pallet_xcm::{Pallet, Event<T>, Origin} = 32,
		DmpQueue: cumulus_pallet_dmp_queue::{Pallet, Call, Storage, Event<T>} = 33,

		// Handy utilities.
		Utility: pallet_utility::{Pallet, Call, Event} = 40,
		Treasury: pallet_treasury::{Pallet, Call, Storage, Config, Event<T>} = 43,
		Proxy: pallet_proxy::{Pallet, Call, Storage, Event<T>} = 44,
		Scheduler: pallet_scheduler::{Pallet, Call, Storage, Event<T>} = 48,

		// Encointer council.
		Collective: pallet_collective::<Instance1>::{Pallet, Call, Storage, Origin<T>, Config<T>, Event<T> } = 50,
		Membership: pallet_membership::<Instance1>::{Pallet, Call, Storage, Event<T>, Config<T>} = 51,

		EncointerScheduler: pallet_encointer_scheduler::{Pallet, Call, Storage, Config<T>, Event} = 60,
		EncointerCeremonies: pallet_encointer_ceremonies::{Pallet, Call, Storage, Config<T>, Event<T>} = 61,
		EncointerCommunities: pallet_encointer_communities::{Pallet, Call, Storage, Config<T>, Event<T>} = 62,
		EncointerBalances: pallet_encointer_balances::{Pallet, Call, Storage, Config<T>, Event<T>} = 63,
		EncointerBazaar: pallet_encointer_bazaar::{Pallet, Call, Storage, Event<T>} = 64,
		EncointerReputationCommitments: pallet_encointer_reputation_commitments::{Pallet, Call, Storage, Event<T>} = 65,
		EncointerFaucet: pallet_encointer_faucet::{Pallet, Call, Storage, Config<T>, Event<T>} = 66,
	}
}

/// `parachains_common` is an upstream crate, where they are started to define common types.
///
/// The re-export is added by encointer.
pub use parachains_common::{AccountId, Balance, BlockNumber, Hash, Header, Index, Signature};

/// The address format for describing accounts.
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;
/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;
/// The SignedExtension to the basic transaction logic.
pub type SignedExtra = (
	frame_system::CheckNonZeroSender<Runtime>,
	frame_system::CheckSpecVersion<Runtime>,
	frame_system::CheckTxVersion<Runtime>,
	frame_system::CheckGenesis<Runtime>,
	frame_system::CheckEra<Runtime>,
	frame_system::CheckNonce<Runtime>,
	frame_system::CheckWeight<Runtime>,
	pallet_asset_tx_payment::ChargeAssetTxPayment<Runtime>,
);
/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
/// Extrinsic type that has already been checked.
pub type CheckedExtrinsic = generic::CheckedExtrinsic<AccountId, RuntimeCall, SignedExtra>;
/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
	Runtime,
	Block,
	frame_system::ChainContext<Runtime>,
	Runtime,
	AllPalletsWithSystem,
	(
		// can migrate from v0 or v1 to v2
		pallet_encointer_communities::migrations::v2::MigrateV0orV1toV2<Runtime>,
		// expected to be noop. but need to try-runtime checks first!
		pallet_encointer_ceremonies::migrations::v1::MigrateToV1<Runtime>,
	),
>;

#[cfg(feature = "runtime-benchmarks")]
#[macro_use]
extern crate frame_benchmarking;

#[cfg(feature = "runtime-benchmarks")]
mod benches {
	define_benchmarks!(
		[frame_system, SystemBench::<Runtime>]
		[pallet_balances, Balances]
		[pallet_collective, Collective]
		[pallet_membership, Membership]
		[pallet_timestamp, Timestamp]
		[pallet_treasury, Treasury]
		[pallet_utility, Utility]
		[pallet_proxy, Proxy]
		[pallet_encointer_balances, EncointerBalances]
		[pallet_encointer_bazaar, EncointerBazaar]
		[pallet_encointer_ceremonies, EncointerCeremonies]
		[pallet_encointer_communities, EncointerCommunities]
		[pallet_encointer_faucet, EncointerFaucet]
		[pallet_encointer_reputation_commitments, EncointerReputationCommitments]
		[pallet_encointer_scheduler, EncointerScheduler]
		[cumulus_pallet_xcmp_queue, XcmpQueue]
	);
}

impl_runtime_apis! {
	impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
		fn slot_duration() -> sp_consensus_aura::SlotDuration {
			sp_consensus_aura::SlotDuration::from_millis(Aura::slot_duration())
		}

		fn authorities() -> Vec<AuraId> {
			Aura::authorities().into_inner()
		}
	}

	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			VERSION
		}

		fn execute_block(block: Block) {
			Executive::execute_block(block)
		}

		fn initialize_block(header: &<Block as BlockT>::Header) {
			Executive::initialize_block(header)
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			OpaqueMetadata::new(Runtime::metadata().into())
		}
		fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
			Runtime::metadata_at_version(version)
		}

		fn metadata_versions() -> sp_std::vec::Vec<u32> {
			Runtime::metadata_versions()
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			Executive::apply_extrinsic(extrinsic)
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			Executive::finalize_block()
		}

		fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			data.create_extrinsics()
		}

		fn check_inherents(
			block: Block,
			data: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			data.check_extrinsics(&block)
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			source: TransactionSource,
			tx: <Block as BlockT>::Extrinsic,
			block_hash: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			Executive::validate_transaction(source, tx, block_hash)
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(header: &<Block as BlockT>::Header) {
			Executive::offchain_worker(header)
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
			SessionKeys::generate(seed)
		}

		fn decode_session_keys(
			encoded: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
			SessionKeys::decode_into_raw_public_keys(&encoded)
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Index> for Runtime {
		fn account_nonce(account: AccountId) -> Index {
			System::account_nonce(account)
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
		fn query_info(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
			TransactionPayment::query_info(uxt, len)
		}
		fn query_fee_details(
			uxt: <Block as BlockT>::Extrinsic,
			len: u32,
		) -> pallet_transaction_payment::FeeDetails<Balance> {
			TransactionPayment::query_fee_details(uxt, len)
		}
		fn query_weight_to_fee(weight: Weight) -> Balance {
			TransactionPayment::weight_to_fee(weight)
	}
		fn query_length_to_fee(length: u32) -> Balance {
			TransactionPayment::length_to_fee(length)
		}
	}

	impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
		fn collect_collation_info(header: &<Block as BlockT>::Header) -> cumulus_primitives_core::CollationInfo {
			ParachainSystem::collect_collation_info(header)
		}
	}

	impl pallet_encointer_ceremonies_rpc_runtime_api::CeremoniesApi<Block, AccountId, Moment> for Runtime {
		fn get_reputations(account: &AccountId) -> Vec<(CeremonyIndexType, CommunityReputation)> {
			EncointerCeremonies::get_reputations(account)
		}
		fn get_aggregated_account_data(cid:CommunityIdentifier, account: &AccountId) -> AggregatedAccountData<AccountId, Moment> {
			EncointerCeremonies::get_aggregated_account_data(cid, account)
		}
		fn get_ceremony_info() -> CeremonyInfo {
			EncointerCeremonies::get_ceremony_info()
		}
	}

	impl pallet_encointer_communities_rpc_runtime_api::CommunitiesApi<Block, AccountId, BlockNumber> for Runtime {
		fn get_all_balances(account: &AccountId) -> Vec<(CommunityIdentifier, BalanceEntry<BlockNumber>)> {
			EncointerCommunities::get_all_balances(account)
		}

		fn get_cids() -> Vec<CommunityIdentifier> {
			EncointerCommunities::get_cids()
		}

		fn get_name(cid: &CommunityIdentifier) -> Option<PalletString> {
			EncointerCommunities::get_name(cid)
		}

		fn get_locations(cid: &CommunityIdentifier) -> Vec<Location> {
			EncointerCommunities::get_locations(cid)
		}
	}

	impl pallet_encointer_bazaar_rpc_runtime_api::BazaarApi<Block, AccountId> for Runtime {
		fn get_offerings(business: &BusinessIdentifier<AccountId>) -> Vec<OfferingData>{
			EncointerBazaar::get_offerings(business)
		}

		fn get_businesses(community: &CommunityIdentifier) -> Vec<(AccountId, BusinessData)>{
			EncointerBazaar::get_businesses(community)
		}
	}

	impl encointer_balances_tx_payment_rpc_runtime_api::BalancesTxPaymentApi<Block, Balance, AssetId, AssetBalance> for Runtime {
		fn balance_to_asset_balance(amount: Balance, asset_id: AssetId) -> Result<AssetBalance, encointer_balances_tx_payment_rpc_runtime_api::Error> {
			BalanceToCommunityBalance::<Runtime>::to_asset_balance(amount, asset_id).map_err(|_e|
				encointer_balances_tx_payment_rpc_runtime_api::Error::RuntimeError
			)
		}
	}

	#[cfg(feature = "try-runtime")]
	impl frame_try_runtime::TryRuntime<Block> for Runtime {
		fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
			// NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
			// have a backtrace here. If any of the pre/post migration checks fail, we shall stop
			// right here and right now.
			let weight = Executive::try_runtime_upgrade(checks).unwrap();
			(weight, RuntimeBlockWeights::get().max_block)
		}

		fn execute_block(
			block: Block,
			state_root_check: bool,
			signature_check: bool,
			select: frame_try_runtime::TryStateSelect
		) -> Weight {
			// NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
			// have a backtrace here.
			Executive::try_execute_block(block, state_root_check, signature_check, select).expect("execute-block failed")
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	impl frame_benchmarking::Benchmark<Block> for Runtime {
		fn benchmark_metadata(extra: bool) -> (
			Vec<frame_benchmarking::BenchmarkList>,
			Vec<frame_support::traits::StorageInfo>,
		) {
			use frame_benchmarking::{Benchmarking, BenchmarkList};
			use frame_support::traits::StorageInfoTrait;
			use frame_system_benchmarking::Pallet as SystemBench;

			let mut list = Vec::<BenchmarkList>::new();
			list_benchmarks!(list, extra);

			let storage_info = AllPalletsWithSystem::storage_info();
			return (list, storage_info)
		}

		fn dispatch_benchmark(
			config: frame_benchmarking::BenchmarkConfig
		) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, sp_runtime::RuntimeString> {
			use frame_benchmarking::{Benchmarking, BenchmarkBatch, TrackedStorageKey};

			use frame_system_benchmarking::Pallet as SystemBench;
			impl frame_system_benchmarking::Config for Runtime {}

			let whitelist: Vec<TrackedStorageKey> = vec![
				// Block Number
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac").to_vec().into(),
				// Total Issuance
				hex_literal::hex!("c2261276cc9d1f8598ea4b6a74b15c2f57c875e4cff74148e4628f264b974c80").to_vec().into(),
				// Execution Phase
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef7ff553b5a9862a516939d82b3d3d8661a").to_vec().into(),
				// Event Count
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef70a98fdbe9ce6c55837576c60c7af3850").to_vec().into(),
				// System Events
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7").to_vec().into(),
				// Treasury Account
				hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef7b99d880ec681799c0cf30e8886371da95ecffd7b6c0f78751baa9d281e0bfa3a6d6f646c70792f74727372790000000000000000000000000000000000000000").to_vec().into(),
			];

			let mut batches = Vec::<BenchmarkBatch>::new();
			let params = (&config, &whitelist);
			add_benchmarks!(params, batches);

			if batches.is_empty() { return Err("Benchmark not found for this pallet.".into()) }
			Ok(batches)
		}
	}
}

struct CheckInherents;

impl cumulus_pallet_parachain_system::CheckInherents<Block> for CheckInherents {
	fn check_inherents(
		block: &Block,
		relay_state_proof: &cumulus_pallet_parachain_system::RelayChainStateProof,
	) -> sp_inherents::CheckInherentsResult {
		let relay_chain_slot = relay_state_proof
			.read_slot()
			.expect("Could not read the relay chain slot from the proof");

		let inherent_data =
			cumulus_primitives_timestamp::InherentDataProvider::from_relay_chain_slot_and_duration(
				relay_chain_slot,
				sp_std::time::Duration::from_secs(6),
			)
			.create_inherent_data()
			.expect("Could not create the timestamp inherent data");

		inherent_data.check_extrinsics(block)
	}
}

cumulus_pallet_parachain_system::register_validate_block! {
	Runtime = Runtime,
	BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
	CheckInherents = CheckInherents,
}
