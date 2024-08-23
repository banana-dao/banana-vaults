use crate::{
    error::ContractError,
    msg::{
        AccountQuery, AccountResponse, DepositMsg, DepositQuery, Environment, ExecuteMsg,
        InstantiateMsg, MigrateMsg, ModifyMsg, PositionMsg, QueryMsg, RewardQuery, State,
        StateQuery, Swap, VaultMsg, WhitelistResponse,
    },
    state::{
        Config, ACCOUNTS_PENDING_BURN, ACCOUNTS_PENDING_MINT, ASSETS_PENDING_MINT, CAP_REACHED,
        COMMISSION_RATE, COMMISSION_REWARDS, CONFIG, HALTED, LAST_UPDATE, OPERATOR, OWNER, POOL_ID,
        POSITION_OPEN, SUPPLY, TERMINATED, UNCOMPOUNDED_REWARDS, VAULT_ASSETS, VAULT_DENOM,
        WHITELISTED_DEPOSITORS,
    },
};
use cosmwasm_std::{
    attr, coin, entry_point, to_json_binary, Addr, Attribute, BankMsg, Binary, Coin, Coins,
    CosmosMsg, Decimal, Deps, DepsMut, Empty, Env, MessageInfo, Order, QuerierWrapper, Response,
    StdError, StdResult, Storage, Uint128,
};
use cw2::{get_contract_version, set_contract_version};
use cw_storage_plus::Bound;
use osmosis_std::types::osmosis::{
    concentratedliquidity::v1beta1::{
        ConcentratedliquidityQuerier, FullPositionBreakdown, MsgAddToPosition,
        MsgCollectIncentives, MsgCollectSpreadRewards, MsgCreatePosition, Pool,
        UserPositionsResponse,
    },
    poolmanager::v1beta1::{MsgSplitRouteSwapExactAmountIn, PoolmanagerQuerier},
    tokenfactory::v1beta1::{MsgBurn, MsgCreateDenom, MsgMint},
};
use osmosis_std::types::{
    cosmos::base::v1beta1::Coin as CosmosCoin,
    osmosis::concentratedliquidity::v1beta1::MsgWithdrawPosition,
};
use pyth_sdk_cw::{query_price_feed, PriceIdentifier};
use std::str::FromStr;

// version info for migration info
const CONTRACT_NAME: &str = env!("CARGO_PKG_NAME");
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

// incompatible versions, which should not be migrated from
const INCOMPATIBLE_TAGS: [&str; 4] = ["0.1.0", "0.2.0", "0.3.0", "0.4.0"];

const PYTH_TESTNET_CONTRACT_ADDRESS: &str =
    "osmo1hpdzqku55lmfmptpyj6wdlugqs5etr6teqf7r4yqjjrxjznjhtuqqu5kdh";
const PYTH_MAINNET_CONTRACT_ADDRESS: &str =
    "osmo13ge29x4e2s63a8ytz2px8gurtyznmue4a69n5275692v3qn3ks8q7cwck7";
const PYTH_DUMMY_CONTRACT_ADDRESS: &str = "osmo1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqmcn030";

const BASE_DENOM: &str = "BVT";

const DEC_18: u128 = 1_000_000_000_000_000_000;

// The maximum amount of time that can pass between updates, before the dead man switch is active
const MAX_UPDATE_INTERVAL: u64 = 86400 * 14; // 14 days

// Default minimum amount of coins that can be burned at once
const DEFAULT_MIN_REDEMPTION: Uint128 = Uint128::new(1_000_000);

// Pagination
const MAX_PAGE_LIMIT: u32 = 250;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    // set the owner address as the instantiator
    OWNER.save(deps.storage, &info.sender)?;

    // validate and set the operator address
    deps.api.addr_validate(msg.operator.as_str())?;
    OPERATOR.save(deps.storage, &msg.operator)?;

    let pyth_contract_address = match msg.env {
        Some(Environment::Mainnet) | None => PYTH_MAINNET_CONTRACT_ADDRESS,
        Some(Environment::Testnet) => PYTH_TESTNET_CONTRACT_ADDRESS,
        Some(Environment::Testtube) => PYTH_DUMMY_CONTRACT_ADDRESS,
    };

    let config = Config {
        metadata: msg.metadata,
        min_asset0: msg.min_asset0,
        min_asset1: msg.min_asset1,
        min_redemption: msg.min_redemption,
        dollar_cap: msg.dollar_cap,
        pyth_contract_address: Addr::unchecked(pyth_contract_address),
        price_expiry: msg.price_expiry,
        commission_receiver: msg
            .commission_receiver
            .unwrap_or_else(|| info.sender.clone()),
    };

    // Check that the pool is the correct type and has the correct assets
    verify_pool(
        &deps.as_ref(),
        msg.pool_id,
        msg.asset0.denom.clone(),
        msg.asset1.denom.clone(),
    )?;

    POOL_ID.save(deps.storage, &msg.pool_id)?;
    VAULT_ASSETS.save(deps.storage, &(msg.asset0.clone(), msg.asset1.clone()))?;

    // Check that funds sent match with config
    verify_mint_funds(
        &info.funds,
        msg.asset0.denom.clone(),
        msg.asset1.denom.clone(),
        &msg.min_asset0,
        &msg.min_asset1,
    )?;

    CONFIG.save(deps.storage, &config)?;

    let vault_denom = format!("factory/{}/{}", &env.contract.address, BASE_DENOM);
    VAULT_DENOM.save(deps.storage, &vault_denom)?;

    // create the vault token
    let create_msg: CosmosMsg = MsgCreateDenom {
        sender: env.contract.address.clone().into_string(),
        subdenom: BASE_DENOM.to_string(),
    }
    .into();

    // todo: refine initial mint logic
    // provisional logic
    let initial_mint = Uint128::new(DEC_18);

    let mint_msg: CosmosMsg = MsgMint {
        sender: env.contract.address.clone().into_string(),
        amount: Some(osmosis_std::types::cosmos::base::v1beta1::Coin {
            denom: vault_denom.clone(),
            amount: initial_mint.to_string(),
        }),
        mint_to_address: info.sender.into_string(),
    }
    .into();

    SUPPLY.save(deps.storage, &initial_mint)?;
    LAST_UPDATE.save(deps.storage, &env.block.time.seconds())?;

    CAP_REACHED.save(deps.storage, &false)?;
    HALTED.save(deps.storage, &false)?;
    TERMINATED.save(deps.storage, &false)?;
    POSITION_OPEN.save(deps.storage, &false)?;

    ASSETS_PENDING_MINT.save(
        deps.storage,
        &vec![
            coin(0, msg.asset0.denom.clone()),
            coin(0, msg.asset1.denom.clone()),
        ],
    )?;

    if msg.commission.unwrap_or_default() >= Decimal::percent(100) {
        return Err(ContractError::CommissionTooHigh);
    }
    COMMISSION_RATE.save(deps.storage, &msg.commission.unwrap_or_default())?;
    COMMISSION_REWARDS.save(
        deps.storage,
        &vec![coin(0, msg.asset0.denom), coin(0, msg.asset1.denom)],
    )?;

    UNCOMPOUNDED_REWARDS.save(deps.storage, &vec![])?;

    Ok(Response::new()
        .add_message(create_msg)
        .add_message(mint_msg)
        .add_attribute("action", "banana_vault_instantiate")
        .add_attribute("denom", vault_denom)
        .add_attribute("contract_name", CONTRACT_NAME)
        .add_attribute("contract_version", CONTRACT_VERSION)
        .add_attribute("operator", msg.operator))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::ManageVault(admin_msg) => {
            if info.sender != OWNER.load(deps.storage)?
                && info.sender != OPERATOR.load(deps.storage)?
            {
                return Err(ContractError::Unauthorized);
            }
            match admin_msg {
                VaultMsg::Modify(modify_msg) => match modify_msg {
                    ModifyMsg::Operator(operator) => execute_modify_operator(deps, &operator),
                    ModifyMsg::Config(config) => execute_modify_config(deps, &env, &config),
                    ModifyMsg::PoolId(pool_id) => execute_modify_pool_id(deps, pool_id),
                    ModifyMsg::Commission(commission) => {
                        execute_modify_commission(deps, commission)
                    }
                    ModifyMsg::Whitelist { add, remove } => execute_whitelist(deps, add, remove),
                },
                VaultMsg::CompoundRewards(swap) => execute_compound_rewards(deps, &env, swap),
                VaultMsg::CollectCommission => execute_collect_commission(deps),
                VaultMsg::ProcessMints => execute_process_mints(deps, &env),
                VaultMsg::ProcessBurns => execute_process_burns(deps, &env),
                VaultMsg::Halt => execute_halt(deps),
                VaultMsg::Resume => execute_resume(deps),
            }
        }
        ExecuteMsg::ManagePosition(position_msg) => {
            if info.sender != OPERATOR.load(deps.storage)? {
                return Err(ContractError::Unauthorized);
            }
            if TERMINATED.load(deps.storage)? {
                return Err(ContractError::VaultClosed);
            }
            match position_msg {
                PositionMsg::CreatePosition {
                    lower_tick,
                    upper_tick,
                    tokens_provided,
                    token_min_amount0,
                    token_min_amount1,
                    swap,
                } => execute_create_position(
                    deps,
                    &env,
                    lower_tick,
                    upper_tick,
                    &tokens_provided,
                    token_min_amount0,
                    token_min_amount1,
                    swap,
                ),
                PositionMsg::AddToPosition {
                    position_id,
                    amount0,
                    amount1,
                    token_min_amount0,
                    token_min_amount1,
                    swap,
                    override_uptime,
                } => execute_add_to_position(
                    deps,
                    env,
                    position_id,
                    amount0,
                    amount1,
                    token_min_amount0,
                    token_min_amount1,
                    swap,
                    override_uptime,
                ),
                PositionMsg::WithdrawPosition {
                    position_id,
                    liquidity_amount,
                    override_uptime,
                } => execute_withdraw_position(
                    deps,
                    &env,
                    position_id,
                    liquidity_amount,
                    override_uptime,
                ),
            }
        }
        ExecuteMsg::Deposit(deposit_msg) => match deposit_msg {
            DepositMsg::Mint { min_out } => execute_deposit_for_mint(deps, &info, &min_out),
            DepositMsg::Burn { address, amount } => {
                execute_deposit_for_burn(deps, &env, &info, address, amount)
            }
        },
        ExecuteMsg::Unlock => execute_unlock(deps, &env, &info),
    }
}

fn execute_modify_operator(deps: DepsMut, new_operator: &Addr) -> Result<Response, ContractError> {
    deps.api.addr_validate(new_operator.as_str())?;
    OPERATOR.save(deps.storage, new_operator)?;

    Ok(Response::new()
        .add_attribute("action", "banana_vault_modify_operator")
        .add_attribute("new_operator", new_operator))
}

fn execute_modify_config(
    deps: DepsMut,
    env: &Env,
    new_config: &Config,
) -> Result<Response, ContractError> {
    deps.api
        .addr_validate(new_config.pyth_contract_address.as_str())?;

    if let Some(new_dollar_cap) = new_config.dollar_cap {
        let (asset0, asset1) =
            get_vault_balances(&deps.as_ref(), &env.contract.address.to_string(), true)?;

        let pricing = get_vault_pricing(&deps.as_ref(), env, &asset0.amount, &asset1.amount)?;

        CAP_REACHED.save(deps.storage, &(pricing.total_dollars >= new_dollar_cap))?;
    } else {
        CAP_REACHED.save(deps.storage, &false)?;
    }

    CONFIG.save(deps.storage, new_config)?;

    Ok(Response::new().add_attribute("action", "banana_vault_modify_config"))
}

// We will allow the pool type to change as long as the assets are the same
fn execute_modify_pool_id(deps: DepsMut, new_pool_id: u64) -> Result<Response, ContractError> {
    // no position should be open as the contract needs the correct pool id to force close it
    if POSITION_OPEN.load(deps.storage)? {
        return Err(ContractError::PositionOpen);
    }

    let vault_assets = VAULT_ASSETS.load(deps.storage)?;
    verify_pool(
        &deps.as_ref(),
        new_pool_id,
        vault_assets.0.denom,
        vault_assets.1.denom,
    )?;

    POOL_ID.save(deps.storage, &new_pool_id)?;

    Ok(Response::new()
        .add_attribute("action", "banana_vault_modify_pool_id")
        .add_attribute("new_pool_id", new_pool_id.to_string()))
}

fn execute_modify_commission(
    deps: DepsMut,
    new_commission: Decimal,
) -> Result<Response, ContractError> {
    if new_commission >= Decimal::percent(100) {
        return Err(ContractError::CommissionTooHigh);
    }
    COMMISSION_RATE.save(deps.storage, &new_commission)?;

    Ok(Response::new()
        .add_attribute("action", "banana_vault_modify_commission")
        .add_attribute("new_commission", new_commission.to_string()))
}

fn execute_whitelist(
    deps: DepsMut,
    add: Option<Vec<Addr>>,
    remove: Option<Vec<Addr>>,
) -> Result<Response, ContractError> {
    let mut attributes: Vec<Attribute> = vec![];
    for address in add.unwrap_or_default() {
        deps.api.addr_validate(address.as_str())?;
        if WHITELISTED_DEPOSITORS.has(deps.storage, address.clone()) {
            return Err(ContractError::AddressInWhitelist {
                address: address.to_string(),
            });
        }

        WHITELISTED_DEPOSITORS.save(deps.storage, address.clone(), &Empty {})?;
        attributes.push(attr("action", "banana_vault_whitelist_add"));
        attributes.push(attr("address", address));
    }

    for address in remove.unwrap_or_default() {
        deps.api.addr_validate(address.as_str())?;
        if WHITELISTED_DEPOSITORS.has(deps.storage, address.clone()) {
            WHITELISTED_DEPOSITORS.remove(deps.storage, address.clone());
            attributes.push(attr("action", "banana_vault_whitelist_remove"));
            attributes.push(attr("address", address));
        } else {
            return Err(ContractError::AddressNotInWhitelist {
                address: address.to_string(),
            });
        }
    }

    Ok(Response::new().add_attributes(attributes))
}

fn execute_compound_rewards(
    deps: DepsMut,
    env: &Env,
    swaps: Vec<Swap>,
) -> Result<Response, ContractError> {
    let mut messages: Vec<CosmosMsg> = vec![];
    let mut rewards = Coins::try_from(UNCOMPOUNDED_REWARDS.load(deps.storage)?).unwrap_or_default();

    for swap in swaps {
        let mut total_in = Uint128::zero();

        for route in &swap.routes {
            total_in += Uint128::from_str(&route.token_in_amount)?;
        }

        let available_balance = rewards.amount_of(&swap.token_in_denom);

        // we can't swap more than we have recorded as collected
        if total_in > available_balance {
            return Err(ContractError::CannotSwapMoreThanAvailable {
                denom: swap.token_in_denom,
            });
        } else {
            rewards.sub(coin(total_in.u128(), swap.token_in_denom.clone()))?;
        }

        // make sure the denom out is one of the vault assets
        let vault_assets = VAULT_ASSETS.load(deps.storage)?;

        if let Some(last_pool) = swap.routes.first().and_then(|route| route.pools.last()) {
            if last_pool.token_out_denom != vault_assets.0.denom
                && last_pool.token_out_denom != vault_assets.1.denom
            {
                return Err(ContractError::CannotSwapIntoAsset);
            }
        }

        messages.push(
            MsgSplitRouteSwapExactAmountIn {
                sender: env.contract.address.to_string(),
                routes: swap.routes,
                token_in_denom: swap.token_in_denom,
                token_out_min_amount: swap.token_out_min_amount,
            }
            .into(),
        );
    }

    UNCOMPOUNDED_REWARDS.save(deps.storage, &rewards.to_vec())?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "banana_vault_compound_rewards"))
}

fn execute_deposit_for_mint(
    deps: DepsMut,
    info: &MessageInfo,
    min_out: &Option<Uint128>,
) -> Result<Response, ContractError> {
    // Check if vault is closed
    if TERMINATED.load(deps.storage)? {
        return Err(ContractError::VaultClosed);
    }

    // Check if vault is halted
    if HALTED.load(deps.storage)? {
        return Err(ContractError::VaultHalted);
    }

    // Check if vault cap has been reached and user is not whitelisted to exceed it
    if CAP_REACHED.load(deps.storage)?
        && !WHITELISTED_DEPOSITORS.has(deps.storage, info.sender.clone())
    {
        return Err(ContractError::CapReached);
    }

    // Check if user is already waiting to burn
    if ACCOUNTS_PENDING_BURN.has(deps.storage, info.sender.clone()) {
        return Err(ContractError::AccountPendingBurn {
            address: info.sender.to_string(),
        });
    }

    let vault_assets = VAULT_ASSETS.load(deps.storage)?;
    let config = CONFIG.load(deps.storage)?;
    let mint_assets = verify_mint_funds(
        &info.funds,
        vault_assets.0.denom,
        vault_assets.1.denom,
        &config.min_asset0,
        &config.min_asset1,
    )?;

    // We queue up the assets for the next iteration
    let mut assets_pending = ASSETS_PENDING_MINT.load(deps.storage)?;

    assets_pending[0].amount += mint_assets[0].amount;
    assets_pending[1].amount += mint_assets[1].amount;

    ASSETS_PENDING_MINT.save(deps.storage, &assets_pending)?;

    let min_out = min_out.unwrap_or_default();

    // Check if user added to the current pending amount or if it's the first time he added
    if let Some((mut current_funds, current_min_out)) =
        ACCOUNTS_PENDING_MINT.may_load(deps.storage, info.sender.clone())?
    {
        current_funds[0].amount += mint_assets[0].amount;
        current_funds[1].amount += mint_assets[1].amount;

        ACCOUNTS_PENDING_MINT.save(
            deps.storage,
            info.sender.clone(),
            &(current_funds, current_min_out + min_out),
        )?;
    } else {
        ACCOUNTS_PENDING_MINT.save(
            deps.storage,
            info.sender.clone(),
            &(mint_assets.clone(), min_out),
        )?;
    }

    Ok(Response::new().add_attribute("action", "banana_vault_deposit_for_mint"))
}

fn execute_deposit_for_burn(
    deps: DepsMut,
    env: &Env,
    info: &MessageInfo,
    address: Option<Addr>,
    amount: Option<Uint128>,
) -> Result<Response, ContractError> {
    // Check if vault is halted
    if HALTED.load(deps.storage)? {
        return Err(ContractError::VaultHalted);
    }

    let mut burn_address = info.sender.clone();
    let mut burn_amount = info.funds[0].amount;

    let mut messages = vec![];
    let mut attributes = vec![];

    // If an address is provided, we use that instead of the sender and execute a forced burn
    if let Some(addr) = address {
        if info.sender != OPERATOR.load(deps.storage)? {
            return Err(ContractError::CannotForceExit);
        }

        let msgs = prepare_force_burn(&deps, env, addr.as_str(), amount)?;
        messages.extend(msgs);
        attributes.push(attr("action", "banana_vault_force_burn"));

        burn_address = addr;
        burn_amount = amount.unwrap();
    } else {
        // make sure valid funds are sent
        verify_burn_funds(deps.storage, &info.funds)?;
        attributes.push(attr("action", "banana_vault_deposit_for_burn"));
    }

    attributes.push(attr("address", burn_address.to_string()));
    attributes.push(attr("amount", burn_amount));

    // if this account is already in the burn list, we add the funds to the existing amount
    if let Some(pending_burn) =
        ACCOUNTS_PENDING_BURN.may_load(deps.storage, burn_address.clone())?
    {
        ACCOUNTS_PENDING_BURN.save(
            deps.storage,
            burn_address.clone(),
            &(pending_burn + burn_amount),
        )?;
    } else {
        ACCOUNTS_PENDING_BURN.save(deps.storage, burn_address.clone(), &burn_amount)?;
    }

    // We return any pending joining assets immediately
    if let Some((mut pending_mint, _)) =
        ACCOUNTS_PENDING_MINT.may_load(deps.storage, burn_address.clone())?
    {
        let mut assets_pending = ASSETS_PENDING_MINT.load(deps.storage)?;
        assets_pending[0].amount -= pending_mint[0].amount;
        assets_pending[1].amount -= pending_mint[1].amount;

        ASSETS_PENDING_MINT.save(deps.storage, &assets_pending)?;
        ACCOUNTS_PENDING_MINT.remove(deps.storage, burn_address.clone());

        // Remove empty amounts to avoid sending empty funds in bank msg
        pending_mint.retain(|f| f.amount.ne(&Uint128::zero()));

        messages.push(
            BankMsg::Send {
                to_address: burn_address.to_string(),
                amount: pending_mint,
            }
            .into(),
        );
    }

    // if vault is terminated the burn will be processed immediately
    if TERMINATED.load(deps.storage)? {
        let (burn_msgs, burn_attrs) = process_burns(deps, env)?;
        messages.extend(burn_msgs);
        attributes.extend(burn_attrs);
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

#[allow(clippy::too_many_arguments)]
fn execute_create_position(
    deps: DepsMut,
    env: &Env,
    lower_tick: i64,
    upper_tick: i64,
    tokens_provided: &Vec<Coin>,
    token_min_amount0: String,
    token_min_amount1: String,
    swap: Option<Swap>,
) -> Result<Response, ContractError> {
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;

    if POSITION_OPEN.load(deps.storage)? {
        return Err(ContractError::PositionOpen);
    }

    let mut messages = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    let mut balance_asset0 = deps
        .querier
        .query_balance(env.contract.address.clone(), vault_assets.0.denom.clone())?;
    let mut balance_asset1 = deps
        .querier
        .query_balance(env.contract.address.clone(), vault_assets.1.denom.clone())?;

    // execute swap if provided
    if let Some(swap) = swap {
        let (message, attribute) = prepare_swap(
            &mut balance_asset0,
            &mut balance_asset1,
            env.contract.address.to_string(),
            swap,
        )?;
        messages.push(message);
        attributes.push(attribute);
    }

    verify_availability_of_funds(
        deps.storage,
        tokens_provided,
        balance_asset0.amount,
        balance_asset1.amount,
    )?;

    messages.push(
        MsgCreatePosition {
            pool_id: POOL_ID.load(deps.storage)?,
            sender: env.contract.address.to_string(),
            lower_tick,
            upper_tick,
            tokens_provided: tokens_provided
                .iter()
                .map(|coin| CosmosCoin {
                    denom: coin.denom.to_string(),
                    amount: coin.amount.to_string(),
                })
                .collect(),
            token_min_amount0,
            token_min_amount1,
        }
        .into(),
    );

    attributes.push(attr("action", "banana_vault_create_position"));

    POSITION_OPEN.save(deps.storage, &true)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

#[allow(clippy::too_many_arguments)]
fn execute_add_to_position(
    deps: DepsMut,
    env: Env,
    position_id: u64,
    amount0: String,
    amount1: String,
    token_min_amount0: String,
    token_min_amount1: String,
    swap: Option<Swap>,
    override_uptime: Option<bool>,
) -> Result<Response, ContractError> {
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;

    let contract_address = env.contract.address;
    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    let mut balance_asset0 = deps
        .querier
        .query_balance(contract_address.clone(), vault_assets.0.denom.clone())?;
    let mut balance_asset1 = deps
        .querier
        .query_balance(contract_address.clone(), vault_assets.1.denom.clone())?;

    // Collect rewards instead of letting them be claimed when adding to position
    let rewards = collect_rewards(
        &deps,
        contract_address.to_string(),
        position_id,
        override_uptime.unwrap_or_default(),
    )?;

    UNCOMPOUNDED_REWARDS.save(deps.storage, &rewards.non_vault)?;
    COMMISSION_REWARDS.save(deps.storage, &rewards.commission)?;

    balance_asset0.amount += rewards.amount0;
    balance_asset1.amount += rewards.amount1;

    messages.extend(rewards.messages);
    attributes.extend(rewards.attributes);

    // execute swap if provided
    if let Some(swap) = swap {
        let (message, attribute) = prepare_swap(
            &mut balance_asset0,
            &mut balance_asset1,
            contract_address.to_string(),
            swap,
        )?;
        messages.push(message);
        attributes.push(attribute);
    }

    let tokens_provided = vec![
        coin(amount0.parse::<u128>()?, vault_assets.0.denom),
        coin(amount1.parse::<u128>()?, vault_assets.1.denom),
    ];

    verify_availability_of_funds(
        deps.storage,
        &tokens_provided,
        balance_asset0.amount,
        balance_asset1.amount,
    )?;

    messages.push(
        MsgAddToPosition {
            position_id,
            sender: contract_address.to_string(),
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
        }
        .into(),
    );

    attributes.push(attr("action", "banana_vault_add_to_position"));

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

fn execute_withdraw_position(
    deps: DepsMut,
    env: &Env,
    position_id: u64,
    liquidity_amount: String,
    override_uptime: Option<bool>,
) -> Result<Response, ContractError> {
    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    let rewards = collect_rewards(
        &deps,
        env.contract.address.to_string(),
        position_id,
        override_uptime.unwrap_or_default(),
    )?;

    UNCOMPOUNDED_REWARDS.save(deps.storage, &rewards.non_vault)?;
    COMMISSION_REWARDS.save(deps.storage, &rewards.commission)?;

    let msg_withdraw_position: CosmosMsg = MsgWithdrawPosition {
        position_id,
        sender: env.contract.address.to_string(),
        liquidity_amount,
    }
    .into();

    messages.push(msg_withdraw_position);
    attributes.push(attr("action", "banana_vault_withdraw_position"));

    POSITION_OPEN.save(deps.storage, &false)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

fn execute_process_mints(deps: DepsMut, env: &Env) -> Result<Response, ContractError> {
    let (msgs, attrs) = process_mints(deps, env)?;
    Ok(Response::new().add_messages(msgs).add_attributes(attrs))
}

fn execute_process_burns(deps: DepsMut, env: &Env) -> Result<Response, ContractError> {
    let (msgs, attrs) = process_burns(deps, env)?;
    Ok(Response::new().add_messages(msgs).add_attributes(attrs))
}

fn execute_collect_commission(deps: DepsMut) -> Result<Response, ContractError> {
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;
    let mut commission_rewards = COMMISSION_REWARDS.load(deps.storage)?;

    commission_rewards.retain(|f| !f.amount.is_zero());

    if commission_rewards.is_empty() {
        return Err(ContractError::CannotClaim);
    }

    COMMISSION_REWARDS.save(
        deps.storage,
        &vec![coin(0, vault_assets.0.denom), coin(0, vault_assets.1.denom)],
    )?;

    Ok(Response::new()
        .add_message(BankMsg::Send {
            to_address: CONFIG.load(deps.storage)?.commission_receiver.to_string(),
            amount: commission_rewards,
        })
        .add_attribute("action", "banana_vault_claim_commission"))
}

fn execute_halt(deps: DepsMut) -> Result<Response, ContractError> {
    HALTED.save(deps.storage, &true)?;
    Ok(Response::new().add_attribute("action", "banana_vault_halt"))
}

fn execute_resume(deps: DepsMut) -> Result<Response, ContractError> {
    HALTED.save(deps.storage, &false)?;
    Ok(Response::new().add_attribute("action", "banana_vault_resume"))
}

fn execute_unlock(deps: DepsMut, env: &Env, info: &MessageInfo) -> Result<Response, ContractError> {
    // Only the operator can unlock the vault, unless the vault has not been updated for a long time
    if info.sender != OPERATOR.load(deps.storage)?
        && env.block.time.seconds() < LAST_UPDATE.load(deps.storage)? + MAX_UPDATE_INTERVAL
    {
        return Err(ContractError::CantUnlockYet {
            seconds: LAST_UPDATE.load(deps.storage)? + MAX_UPDATE_INTERVAL
                - env.block.time.seconds(),
        });
    }

    let mut messages = vec![];
    let mut attributes = vec![];

    // We get any addresses that are waiting to mint and send the funds back
    let addresses_pending_activation: Vec<(Addr, (Vec<Coin>, _))> = ACCOUNTS_PENDING_MINT
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(Result::ok)
        .collect();

    for (address, (amount, _)) in &addresses_pending_activation {
        let mut funds = amount.clone();

        // Remove empty amounts to avoid sending empty funds in bank msg
        funds.retain(|f| f.amount.ne(&Uint128::zero()));

        let send_msg = BankMsg::Send {
            to_address: address.to_string(),
            amount: funds,
        };

        messages.push(send_msg.into());
    }

    ACCOUNTS_PENDING_MINT.clear(deps.storage);

    // if an open position exists, close it
    if POSITION_OPEN.load(deps.storage)? {
        let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
        let user_positions_response: UserPositionsResponse = cl_querier.user_positions(
            env.contract.address.to_string(),
            POOL_ID.load(deps.storage)?,
            None,
        )?;

        let position = user_positions_response.positions[0]
            .position
            .as_ref()
            .unwrap();

        let rewards = collect_rewards(
            &deps,
            env.contract.address.to_string(),
            position.position_id,
            true,
        )?;

        UNCOMPOUNDED_REWARDS.save(deps.storage, &rewards.non_vault)?;
        COMMISSION_REWARDS.save(deps.storage, &rewards.commission)?;

        let msg_withdraw_position: CosmosMsg = MsgWithdrawPosition {
            position_id: position.position_id,
            sender: env.contract.address.to_string(),
            liquidity_amount: position.liquidity.clone(),
        }
        .into();

        messages.push(msg_withdraw_position);

        POSITION_OPEN.save(deps.storage, &false)?;
    }

    // set terminated to true and halted to false to allow immediate redemptions
    HALTED.save(deps.storage, &false)?;
    TERMINATED.save(deps.storage, &true)?;
    attributes.push(attr("action", "banana_vault_terminate"));

    // process any pending burns
    let (burn_msgs, burn_attrs) = process_burns(deps, env)?;
    messages.extend(burn_msgs);
    attributes.extend(burn_attrs);

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::EstimateDeposit(deposit_query) => match deposit_query {
            DepositQuery::Mint(coins) => to_json_binary(&query_estimate_mint(deps, &env, &coins)?),
            DepositQuery::Burn(amount) => to_json_binary(&query_estimate_burn(deps, &env, amount)?),
        },
        QueryMsg::LockedAssets => to_json_binary(&query_locked_assets(deps, &env)?),
        QueryMsg::AccountStatus(pending_query) => match pending_query {
            AccountQuery::Mint(mint) => to_json_binary(&query_pending_mint(
                deps,
                mint.address,
                mint.start_after,
                mint.limit,
            )?),
            AccountQuery::Burn(burn) => to_json_binary(&query_pending_burn(
                deps,
                burn.address,
                burn.start_after,
                burn.limit,
            )?),
        },
        QueryMsg::Rewards(reward_query) => match reward_query {
            RewardQuery::Commission => to_json_binary(&query_commission_rewards(deps)?),
            RewardQuery::Uncompounded => to_json_binary(&query_uncompounded_rewards(deps)?),
        },
        QueryMsg::Whitelist { start_after, limit } => {
            to_json_binary(&query_whitelist(deps, start_after, limit))
        }
        QueryMsg::VaultState(state_query) => match state_query {
            StateQuery::Info => to_json_binary(&query_info(deps)?),
            StateQuery::Status => to_json_binary(&query_status(deps, &env)?),
        },
    }
}

fn query_locked_assets(deps: Deps, env: &Env) -> StdResult<Vec<Coin>> {
    let address = env.contract.address.to_string();

    let (asset0, asset1) = get_vault_balances(&deps, &address, true)?;

    Ok(vec![asset0, asset1])
}

fn query_estimate_mint(deps: Deps, env: &Env, coins: &[Coin]) -> StdResult<Vec<Coin>> {
    let config = CONFIG.load(deps.storage)?;
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;

    let mint_funds: Vec<Coin> = verify_mint_funds(
        coins,
        vault_assets.0.denom,
        vault_assets.1.denom,
        &config.min_asset0,
        &config.min_asset1,
    )
    .unwrap();

    let (asset0, asset1) = get_vault_balances(&deps, &env.contract.address.to_string(), true)?;

    let pricing = get_vault_pricing(&deps, env, &asset0.amount, &asset1.amount)?;

    let dollars_asset0 = mint_funds[0].amount.checked_mul(pricing.price0)?;
    let dollars_asset1 = mint_funds[1].amount.checked_mul(pricing.price1)?;

    let total_dollars_to_mint = dollars_asset0.checked_add(dollars_asset1)?;

    Ok(vec![coin(
        total_dollars_to_mint
            .checked_div(pricing.vault_price)?
            .u128(),
        VAULT_DENOM.load(deps.storage)?,
    )])
}

fn query_estimate_burn(deps: Deps, env: &Env, amount: Uint128) -> StdResult<Vec<Coin>> {
    let (asset0, asset1) = get_vault_balances(&deps, &env.contract.address.to_string(), true)?;
    let ratio = Decimal::new(amount)
        .checked_div(Decimal::new(SUPPLY.load(deps.storage)?))
        .unwrap();

    Ok(vec![
        coin(asset0.amount.mul_floor(ratio).u128(), asset0.denom),
        coin(asset1.amount.mul_floor(ratio).u128(), asset1.denom),
    ])
}

fn query_pending_mint(
    deps: Deps,
    address: Option<Addr>,
    start_after: Option<Addr>,
    limit: Option<u32>,
) -> StdResult<Vec<AccountResponse>> {
    let limit = limit.unwrap_or(MAX_PAGE_LIMIT).min(MAX_PAGE_LIMIT);
    let start = start_after.map(Bound::exclusive);
    let pending_mint: Vec<(Addr, (Vec<Coin>, _))> = match address {
        Some(addr) => {
            if let Some((pending, min_out)) =
                ACCOUNTS_PENDING_MINT.may_load(deps.storage, addr.clone())?
            {
                vec![(addr, (pending, min_out))]
            } else {
                vec![]
            }
        }
        None => ACCOUNTS_PENDING_MINT
            .range(deps.storage, start, None, Order::Ascending)
            .take(limit as usize)
            .filter_map(Result::ok)
            .collect(),
    };

    Ok(pending_mint
        .iter()
        .map(|(address, (amount, min_out))| AccountResponse {
            address: address.clone(),
            amount: amount.clone(),
            min_out: Some(*min_out),
        })
        .collect())
}

fn query_pending_burn(
    deps: Deps,
    address: Option<Addr>,
    start_after: Option<Addr>,
    limit: Option<u32>,
) -> StdResult<Vec<AccountResponse>> {
    let limit = limit.unwrap_or(MAX_PAGE_LIMIT).min(MAX_PAGE_LIMIT);
    let start = start_after.map(Bound::exclusive);

    let pending_burn: Vec<(Addr, Uint128)> = match address {
        Some(addr) => {
            if let Some(pending) = ACCOUNTS_PENDING_BURN.may_load(deps.storage, addr.clone())? {
                vec![(addr, pending)]
            } else {
                vec![]
            }
        }
        None => ACCOUNTS_PENDING_BURN
            .range(deps.storage, start, None, Order::Ascending)
            .take(limit as usize)
            .filter_map(Result::ok)
            .collect(),
    };

    let denom = VAULT_DENOM.load(deps.storage)?;

    Ok(pending_burn
        .iter()
        .map(|(address, amount)| AccountResponse {
            address: address.clone(),
            amount: vec![coin(amount.u128(), denom.clone())],
            min_out: None,
        })
        .collect())
}

fn query_commission_rewards(deps: Deps) -> StdResult<Vec<Coin>> {
    COMMISSION_REWARDS.load(deps.storage)
}

fn query_uncompounded_rewards(deps: Deps) -> StdResult<Vec<Coin>> {
    UNCOMPOUNDED_REWARDS.load(deps.storage)
}

fn query_whitelist(deps: Deps, start_after: Option<Addr>, limit: Option<u32>) -> WhitelistResponse {
    let limit = limit.unwrap_or(MAX_PAGE_LIMIT).min(MAX_PAGE_LIMIT);
    let start = start_after.map(Bound::exclusive);
    let whitelisted_depositors: Vec<Addr> = WHITELISTED_DEPOSITORS
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit as usize)
        .filter_map(Result::ok)
        .map(|(addr, _)| addr)
        .collect();

    WhitelistResponse {
        whitelisted_depositors,
    }
}

fn query_info(deps: Deps) -> StdResult<State> {
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;
    let commission_rate = COMMISSION_RATE.load(deps.storage)?;

    Ok(State::Info {
        asset0: vault_assets.0,
        asset1: vault_assets.1,
        pool_id: POOL_ID.load(deps.storage)?,
        owner: OWNER.load(deps.storage)?,
        operator: OPERATOR.load(deps.storage)?,
        commission_rate,
        config: Box::new(CONFIG.load(deps.storage)?),
    })
}

fn query_status(deps: Deps, env: &Env) -> StdResult<State> {
    let mut join_time = 0;
    let mut uptime_locked = false;

    if POSITION_OPEN.load(deps.storage)? {
        let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
        let position = &cl_querier
            .user_positions(
                env.contract.address.to_string(),
                POOL_ID.load(deps.storage)?,
                None,
            )?
            .positions[0];

        join_time = position
            .position
            .as_ref()
            .unwrap()
            .join_time
            .as_ref()
            .unwrap()
            .seconds as u64;

        if !position.forfeited_incentives.is_empty() {
            uptime_locked = true;
        }
    }

    Ok(State::Status {
        join_time,
        uptime_locked,
        last_update: LAST_UPDATE.load(deps.storage)?,
        cap_reached: CAP_REACHED.load(deps.storage)?,
        halted: HALTED.load(deps.storage)?,
        terminated: TERMINATED.load(deps.storage)?,
        supply: SUPPLY.load(deps.storage)?,
        denom: VAULT_DENOM.load(deps.storage)?,
    })
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> StdResult<Response> {
    let version = get_contract_version(deps.storage)?;
    if version.contract != CONTRACT_NAME {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    };
    if INCOMPATIBLE_TAGS.contains(&version.version.as_str()) {
        return Err(StdError::generic_err(
            "Cannot upgrade from incompatible version",
        ));
    }
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::default())
}

// Helpers
fn verify_pool(
    deps: &Deps,
    pool_id: u64,
    denom0: String,
    denom1: String,
) -> Result<(), ContractError> {
    let pool: Pool = match PoolmanagerQuerier::new(&deps.querier).pool(pool_id)?.pool {
        Some(pool) => {
            if pool
                .type_url
                .ne(&"/osmosis.concentratedliquidity.v1beta1.Pool".to_string())
            {
                return Err(ContractError::PoolIsNotCL);
            }
            prost::Message::decode(pool.value.as_slice()).unwrap()
        }
        None => {
            return Err(ContractError::PoolNotFound { pool_id });
        }
    };

    if denom0 != pool.token0 {
        return Err(ContractError::InvalidConfigAsset { asset: 0 });
    }

    if denom1 != pool.token1 {
        return Err(ContractError::InvalidConfigAsset { asset: 1 });
    }

    Ok(())
}

fn verify_mint_funds(
    funds: &[Coin],
    denom0: String,
    denom1: String,
    min0: &Uint128,
    min1: &Uint128,
) -> Result<Vec<Coin>, ContractError> {
    if funds.is_empty() {
        return Err(ContractError::NoFunds);
    }

    let mut assets = vec![coin(0, denom0.clone()), coin(0, denom1.clone())];

    for fund in funds {
        if fund.denom == denom0 {
            check_deposit_min(min0, fund)?;
            assets[0].amount = fund.amount;
        } else if fund.denom == denom1 {
            check_deposit_min(min1, fund)?;
            assets[1].amount = fund.amount;
        } else {
            return Err(ContractError::InvalidMintAssets);
        }
    }

    Ok(assets)
}

fn check_deposit_min(min_deposit: &Uint128, coin: &Coin) -> Result<(), ContractError> {
    if min_deposit.is_zero() {
        return Err(ContractError::DepositNotAllowed {
            denom: coin.denom.clone(),
        });
    }
    if coin.amount.lt(min_deposit) {
        return Err(ContractError::DepositBelowMinimum {
            denom: coin.denom.clone(),
        });
    }

    Ok(())
}

fn verify_burn_funds(storage: &dyn Storage, funds: &[Coin]) -> Result<(), ContractError> {
    if funds.is_empty() {
        return Err(ContractError::NoFunds);
    }

    let denom = VAULT_DENOM.load(storage)?;

    if funds.len() > 1 || funds[0].denom != denom {
        return Err(ContractError::InvalidToken { denom });
    }

    let min_redemption = CONFIG
        .load(storage)?
        .min_redemption
        .unwrap_or(DEFAULT_MIN_REDEMPTION);

    if funds[0].amount < min_redemption {
        Err(ContractError::RedemptionBelowMinimum {
            min: min_redemption.to_string(),
        })
    } else {
        Ok(())
    }
}

fn verify_availability_of_funds(
    storage: &dyn Storage,
    tokens_provided: &Vec<Coin>,
    amount_asset0: Uint128,
    amount_asset1: Uint128,
) -> Result<(), ContractError> {
    let assets_pending = ASSETS_PENDING_MINT.load(storage)?;
    let commissions = COMMISSION_REWARDS.load(storage)?;

    let reserved_assets = [
        Coin {
            denom: assets_pending[0].denom.clone(),
            amount: assets_pending[0].amount + commissions[0].amount,
        },
        Coin {
            denom: assets_pending[1].denom.clone(),
            amount: assets_pending[1].amount + commissions[1].amount,
        },
    ];

    let vault_assets = VAULT_ASSETS.load(storage)?;

    for token_provided in tokens_provided {
        if token_provided.denom == vault_assets.0.denom
            && token_provided.amount > amount_asset0.checked_sub(reserved_assets[0].amount)?
        {
            return Err(ContractError::CannotAddMoreThanAvailableForAsset {
                asset: vault_assets.0.denom,
                amount: amount_asset0
                    .checked_sub(reserved_assets[0].amount)?
                    .to_string(),
            });
        }
        if token_provided.denom == vault_assets.1.denom
            && token_provided.amount > amount_asset1.checked_sub(reserved_assets[1].amount)?
        {
            return Err(ContractError::CannotAddMoreThanAvailableForAsset {
                asset: vault_assets.1.denom,
                amount: amount_asset1
                    .checked_sub(reserved_assets[1].amount)?
                    .to_string(),
            });
        }
    }

    Ok(())
}

// Asset0 and Asset1 in the vault, minus pending assets and commissions
fn get_vault_balances(
    deps: &Deps,
    address: &String,
    include_position: bool,
) -> Result<(Coin, Coin), StdError> {
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;
    let mut asset0 = deps
        .querier
        .query_balance(address.clone(), vault_assets.0.denom.clone())?;
    let mut asset1 = deps
        .querier
        .query_balance(address, vault_assets.1.denom.clone())?;

    if POSITION_OPEN.load(deps.storage)? && include_position {
        let commission_remainder = Decimal::one() - COMMISSION_RATE.load(deps.storage)?;

        let position = &ConcentratedliquidityQuerier::new(&deps.querier)
            .user_positions(address.to_string(), POOL_ID.load(deps.storage)?, None)?
            .positions[0];

        asset0.amount = asset0.amount.checked_add(
            position
                .asset0
                .clone()
                .unwrap_or_default()
                .amount
                .parse::<Uint128>()?,
        )?;
        asset1.amount = asset1.amount.checked_add(
            position
                .asset1
                .clone()
                .unwrap_or_default()
                .amount
                .parse::<Uint128>()?,
        )?;

        if !position.claimable_incentives.is_empty() {
            for incentive in &position.claimable_incentives {
                if incentive.denom == vault_assets.0.denom {
                    asset0.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                } else if incentive.denom == vault_assets.1.denom {
                    asset1.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                }
            }
        }

        if !position.claimable_spread_rewards.is_empty() {
            for incentive in &position.claimable_spread_rewards {
                if incentive.denom == vault_assets.0.denom {
                    asset0.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                } else if incentive.denom == vault_assets.1.denom {
                    asset1.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                }
            }
        }
    }

    let assets_pending = ASSETS_PENDING_MINT.load(deps.storage)?;
    let commissions = COMMISSION_REWARDS.load(deps.storage)?;

    Ok((
        coin(
            asset0
                .amount
                .checked_sub(assets_pending[0].amount)?
                .checked_sub(commissions[0].amount)?
                .u128(),
            vault_assets.0.denom,
        ),
        coin(
            asset1
                .amount
                .checked_sub(assets_pending[1].amount)?
                .checked_sub(commissions[1].amount)?
                .u128(),
            vault_assets.1.denom,
        ),
    ))
}

struct Rewards {
    amount0: Uint128,
    amount1: Uint128,
    non_vault: Vec<Coin>,
    commission: Vec<Coin>,
    messages: Vec<CosmosMsg>,
    attributes: Vec<Attribute>,
}

// collect_rewards should be called upon any position change
// however we only need to actually execute the msgs when adding to position
fn collect_rewards(
    deps: &DepsMut,
    sender: String,
    position_id: u64,
    override_uptime: bool,
) -> Result<Rewards, ContractError> {
    let position: FullPositionBreakdown = match ConcentratedliquidityQuerier::new(&deps.querier)
        .position_by_id(position_id)?
        .position
    {
        Some(position) => position,
        None => {
            return Err(ContractError::NoPositionsOpen);
        }
    };

    let mut reward_coins: Coins = Coins::default();
    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    // if there's forfeited incentives in the response, min uptime for that incentive has yet not been met.
    // can be overridden by the operator to allow forfeiture if repositioning is more advantageous
    if !position.forfeited_incentives.is_empty() && !override_uptime {
        return Err(ContractError::MinUptime);
    }

    if !position.claimable_incentives.is_empty() {
        for incentive in &position.claimable_incentives {
            reward_coins.add(coin(
                Uint128::from_str(&incentive.amount)?.u128(),
                incentive.denom.clone(),
            ))?;
        }
        messages.push(
            MsgCollectIncentives {
                position_ids: vec![position_id],
                sender: sender.clone(),
            }
            .into(),
        );
    }

    if !position.claimable_spread_rewards.is_empty() {
        for incentive in &position.claimable_spread_rewards {
            reward_coins.add(coin(
                Uint128::from_str(&incentive.amount)?.u128(),
                incentive.denom.clone(),
            ))?;
        }
        messages.push(
            MsgCollectSpreadRewards {
                position_ids: vec![position_id],
                sender,
            }
            .into(),
        );
    }

    if !reward_coins.is_empty() {
        attributes.push(attr("action", "banana_vault_collect_rewards"));
    }

    let mut amount0 = Uint128::zero();
    let mut amount1 = Uint128::zero();

    // we don't load these as cosmwasm_std::Coins so that they don't get sorted
    let mut commission_coins = COMMISSION_REWARDS.load(deps.storage)?;

    let mut uncompounded_rewards: Coins =
        Coins::try_from(UNCOMPOUNDED_REWARDS.load(deps.storage)?).unwrap_or_default();

    let commission_rate = COMMISSION_RATE.load(deps.storage)?;
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;

    for reward_coin in &reward_coins {
        let commission_amount = reward_coin.amount.mul_floor(commission_rate);

        // compoundable rewards. add commission to the tracker
        if reward_coin.denom == vault_assets.0.denom {
            commission_coins[0].amount += commission_amount;
            amount0 = reward_coin.amount;
        } else if reward_coin.denom == vault_assets.1.denom {
            commission_coins[1].amount += commission_amount;
            amount1 = reward_coin.amount;

        // uncompounded rewards. commission will be deducted when the rewards are distributed
        } else {
            uncompounded_rewards.add(reward_coin.clone())?;
        }
    }

    Ok(Rewards {
        amount0,
        amount1,
        non_vault: uncompounded_rewards.into(),
        commission: commission_coins,
        messages,
        attributes,
    })
}

// this function mutates asset0 and asset1 to reflect the new state after the swap
fn prepare_swap(
    asset0: &mut Coin,
    asset1: &mut Coin,
    sender: String,
    swap: Swap,
) -> Result<(CosmosMsg, Attribute), ContractError> {
    let mut token_in_amount: Uint128 = Uint128::zero();
    for route in &swap.routes {
        token_in_amount += Uint128::from_str(&route.token_in_amount)?;
    }

    // We are not allowed to swap more than what we have currently liquid in the vault
    if swap.token_in_denom == asset0.denom {
        if token_in_amount > asset0.amount {
            return Err(ContractError::CannotSwapMoreThanAvailable {
                denom: asset0.denom.clone(),
            });
        }
        asset0.amount = asset0.amount.checked_sub(token_in_amount)?;
    }

    if swap.token_in_denom == asset1.denom {
        if token_in_amount > asset1.amount {
            return Err(ContractError::CannotSwapMoreThanAvailable {
                denom: asset1.denom.clone(),
            });
        }
        asset1.amount = asset1.amount.checked_sub(token_in_amount)?;
    }

    // every route will end in the same denom. we get the final denom from the first route and add the min amount out to the swap output asset
    if let Some(last_pool) = swap.routes.first().and_then(|route| route.pools.last()) {
        let token_out_min_amount = Uint128::from_str(&swap.token_out_min_amount)?;
        if last_pool.token_out_denom == asset0.denom {
            asset0.amount += token_out_min_amount;
        } else if last_pool.token_out_denom == asset1.denom {
            asset1.amount += token_out_min_amount;
        }
    }

    let msg_split_route_swap_exact_amount_in: CosmosMsg = MsgSplitRouteSwapExactAmountIn {
        sender,
        routes: swap.routes,
        token_in_denom: swap.token_in_denom,
        token_out_min_amount: swap.token_out_min_amount,
    }
    .into();

    Ok((
        msg_split_route_swap_exact_amount_in,
        attr("action", "banana_vault_swap"),
    ))
}

fn prepare_force_burn(
    deps: &DepsMut,
    env: &Env,
    target_address: &str,
    amount: Option<Uint128>,
) -> Result<Vec<CosmosMsg>, ContractError> {
    let amount = amount.unwrap_or_default();

    if amount.is_zero() {
        return Err(ContractError::InsufficientFundsToBurn);
    }

    let token_amount = coin(amount.u128(), VAULT_DENOM.load(deps.storage)?);

    // make sure the address has the tokens to be burned
    if deps
        .querier
        .query_balance(target_address.to_string(), VAULT_DENOM.load(deps.storage)?)?
        .amount
        < amount
    {
        return Err(ContractError::InsufficientFundsToBurn);
    }

    // burn the tokens from the target address and re-mint them to the vault
    let msg_burn = MsgBurn {
        sender: env.contract.address.to_string(),
        amount: Some(token_amount.clone().into()),
        burn_from_address: target_address.to_string(),
    };

    let msg_mint = MsgMint {
        sender: env.contract.address.to_string(),
        amount: Some(token_amount.into()),
        mint_to_address: env.contract.address.to_string(),
    };

    Ok(vec![msg_burn.into(), msg_mint.into()])
}

fn process_mints(
    deps: DepsMut,
    env: &Env,
) -> Result<(Vec<CosmosMsg>, Vec<Attribute>), ContractError> {
    let entries: Vec<(Addr, (Vec<Coin>, Uint128))> = ACCOUNTS_PENDING_MINT
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(Result::ok)
        .collect();

    if entries.is_empty() {
        return Ok((vec![], vec![]));
    }

    let config = CONFIG.load(deps.storage)?;

    let (asset0, asset1) =
        get_vault_balances(&deps.as_ref(), &env.contract.address.to_string(), true)?;

    let pricing = get_vault_pricing(&deps.as_ref(), env, &asset0.amount, &asset1.amount)?;

    let mut total_dollars_in_vault = pricing.total_dollars;

    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes = vec![attr("action", "banana_vault_mint")];

    let mut total_minted = Uint128::zero();
    let mut pending_assets = ASSETS_PENDING_MINT.load(deps.storage)?;

    // for each account to mint we will calculate their dollar value to determine the amount of tokens to mint
    for (address, (coins, min_out)) in entries {
        let dollars_asset0 = coins[0].amount.checked_mul(pricing.price0)?;

        let dollars_asset1 = coins[1].amount.checked_mul(pricing.price1)?;

        let total_dollars_address = dollars_asset0.checked_add(dollars_asset1)?;

        let to_mint = total_dollars_address
            .checked_div(pricing.vault_price)
            .unwrap();

        attributes.push(attr("address", address.to_string()));
        attributes.push(attr("minted", to_mint.to_string()));
        attributes.push(attr("deposited", format!("{},{}", coins[0], coins[1])));

        // we only process the mint if it's within the user's defined slippage, however in the case that
        // min_out is set to 0 and 0 tokens are minted the deposit will be taken uncredited
        if to_mint >= min_out {
            pending_assets[0].amount -= coins[0].amount;
            pending_assets[1].amount -= coins[1].amount;
            ACCOUNTS_PENDING_MINT.remove(deps.storage, address.clone());

            if !to_mint.is_zero() {
                messages.push(
                    MsgMint {
                        sender: env.contract.address.to_string(),
                        amount: Some(osmosis_std::types::cosmos::base::v1beta1::Coin {
                            denom: VAULT_DENOM.load(deps.storage)?,
                            amount: to_mint.to_string(),
                        }),
                        mint_to_address: address.to_string(),
                    }
                    .into(),
                );
            }

        // otherwise we skip processing of this mint and leave it in queue
        } else {
            attributes.push(attr("slippage", format!("\"{to_mint}\",\"{min_out}\"")));
        }

        total_minted += to_mint;
        total_dollars_in_vault = total_dollars_in_vault.checked_add(total_dollars_address)?;
    }

    attributes.push(attr("total_minted", total_minted.to_string()));

    // update total supply of vault tokens with new mints
    let supply = SUPPLY.load(deps.storage)?;
    SUPPLY.save(deps.storage, &(supply.checked_add(total_minted)?))?;

    ASSETS_PENDING_MINT.save(deps.storage, &pending_assets)?;

    // Check that we are not over the vault cap, if that's the case, we will flag it to halt joins until under cap again
    if let Some(dollar_cap) = config.dollar_cap {
        CAP_REACHED.save(deps.storage, &(total_dollars_in_vault >= dollar_cap))?;
    }

    Ok((messages, attributes))
}

fn process_burns(
    deps: DepsMut,
    env: &Env,
) -> Result<(Vec<CosmosMsg>, Vec<Attribute>), ContractError> {
    let exits: Vec<(Addr, Uint128)> = ACCOUNTS_PENDING_BURN
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(Result::ok)
        .collect();

    if exits.is_empty() {
        return Ok((vec![], vec![]));
    }

    let (total_asset0, total_asset1) =
        get_vault_balances(&deps.as_ref(), &env.contract.address.to_string(), true)?;

    let vault_assets = VAULT_ASSETS.load(deps.storage)?;

    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes = vec![attr("action", "banana_vault_burn")];

    let mut total_burned = Uint128::zero();
    let mut distributed_vault_tokens = [
        coin(0, vault_assets.0.denom.clone()),
        coin(0, vault_assets.1.denom.clone()),
    ];
    let supply = SUPPLY.load(deps.storage)?;

    // for each address waiting for burn, calculate the funds to to withdraw
    for (address, to_burn) in &exits {
        let ratio = Decimal::new(*to_burn).checked_div(Decimal::new(supply))?;
        let mut amount_to_send = vec![
            coin(0, vault_assets.0.denom.clone()),
            coin(0, vault_assets.1.denom.clone()),
        ];

        let amount_to_send_asset0 = total_asset0.amount.mul_floor(ratio);
        let amount_to_send_asset1 = total_asset1.amount.mul_floor(ratio);

        distributed_vault_tokens[0].amount += amount_to_send_asset0;
        amount_to_send[0].amount += amount_to_send_asset0;

        distributed_vault_tokens[1].amount += amount_to_send_asset1;
        amount_to_send[1].amount += amount_to_send_asset1;

        amount_to_send.retain(|c| !c.amount.is_zero());

        // note: in the case that 0 tokens are withdrawn, the vault tokens will still be burned
        if !amount_to_send.is_empty() {
            messages.push(
                BankMsg::Send {
                    to_address: address.to_string(),
                    amount: amount_to_send.clone(),
                }
                .into(),
            );
        }

        attributes.push(attr("address", address.to_string()));
        attributes.push(attr("burned", to_burn.to_string()));
        for amount in amount_to_send {
            attributes.push(attr("received", format!("{}", amount)));
        }

        total_burned += to_burn;
    }

    let (liquid_asset0, liquid_asset1) =
        get_vault_balances(&deps.as_ref(), &env.contract.address.to_string(), false)?;
    if distributed_vault_tokens[0].amount > liquid_asset0.amount
        || distributed_vault_tokens[1].amount > liquid_asset1.amount
    {
        return Err(ContractError::CantProcessBurn);
    }

    if !total_burned.is_zero() {
        messages.push(
            MsgBurn {
                sender: env.contract.address.to_string(),
                amount: Some(osmosis_std::types::cosmos::base::v1beta1::Coin {
                    denom: VAULT_DENOM.load(deps.storage)?,
                    amount: total_burned.to_string(),
                }),
                burn_from_address: env.contract.address.to_string(),
            }
            .into(),
        );
    }

    attributes.push(attr("total_burned", total_burned.to_string()));

    // remove burned tokens from the supply
    SUPPLY.save(deps.storage, &(supply.checked_sub(total_burned)?))?;

    // clear the pending accounts
    ACCOUNTS_PENDING_BURN.clear(deps.storage);

    // if all tokens are burned, we can close the vault
    if supply == total_burned {
        TERMINATED.save(deps.storage, &true)?;

    // otherwise check if we are back under the deposit cap
    } else if let Some(dollar_cap) = CONFIG.load(deps.storage)?.dollar_cap {
        let (current_price_asset0, current_price_asset1) = get_asset_prices(&deps.as_ref(), env)?;

        let dollars_asset0 = (total_asset0
            .amount
            .checked_sub(distributed_vault_tokens[0].amount)?)
        .checked_mul(current_price_asset0)?;

        let dollars_asset1 = (total_asset1
            .amount
            .checked_sub(distributed_vault_tokens[1].amount)?)
        .checked_mul(current_price_asset1)?;

        CAP_REACHED.save(
            deps.storage,
            &(dollars_asset0.checked_add(dollars_asset1)? >= dollar_cap),
        )?;
    }

    Ok((messages, attributes))
}

struct Pricing {
    total_dollars: Uint128,
    price0: Uint128,
    price1: Uint128,
    vault_price: Uint128,
}

fn get_vault_pricing(
    deps: &Deps,
    env: &Env,
    amount0: &Uint128,
    amount1: &Uint128,
) -> StdResult<Pricing> {
    let (price0, price1) = get_asset_prices(deps, env)?;

    let dollars0 = amount0.checked_mul(price0)?;
    let dollars1 = amount1.checked_mul(price1)?;

    let total_dollars = dollars0.checked_add(dollars1)?;

    Ok(Pricing {
        total_dollars,
        price0,
        price1,
        vault_price: total_dollars.checked_div(SUPPLY.load(deps.storage)?)?,
    })
}

// gets the up-to-date prices for each vault asset
fn get_asset_prices(deps: &Deps, env: &Env) -> StdResult<(Uint128, Uint128)> {
    let config = CONFIG.load(deps.storage)?;
    let vault_assets = VAULT_ASSETS.load(deps.storage)?;
    let current_time = env.block.time.seconds();

    let price_querier: &dyn PriceQuerier =
        if config.pyth_contract_address == Addr::unchecked(PYTH_DUMMY_CONTRACT_ADDRESS) {
            &MockPriceQuerier
        } else {
            &PythQuerier
        };

    let current_price_asset0 = price_querier.query_asset_price(
        &deps.querier,
        config.pyth_contract_address.clone(),
        vault_assets.0.price_identifier,
        current_time as i64,
        config.price_expiry,
        vault_assets.0.decimals,
    )?;

    let current_price_asset1 = price_querier.query_asset_price(
        &deps.querier,
        config.pyth_contract_address,
        vault_assets.1.price_identifier,
        current_time as i64,
        config.price_expiry,
        vault_assets.1.decimals,
    )?;

    Ok((current_price_asset0, current_price_asset1))
}

pub trait PriceQuerier {
    fn query_asset_price(
        &self,
        querier: &QuerierWrapper,
        contract_address: Addr,
        identifier: PriceIdentifier,
        time: i64,
        expiry: u64,
        exponent: u32,
    ) -> StdResult<Uint128>;
}

struct PythQuerier;
impl PriceQuerier for PythQuerier {
    fn query_asset_price(
        &self,
        querier: &QuerierWrapper,
        contract_address: Addr,
        identifier: PriceIdentifier,
        time: i64,
        expiry: u64,
        exponent: u32,
    ) -> StdResult<Uint128> {
        query_price_feed(querier, contract_address, identifier)?
            .price_feed
            .get_price_no_older_than(time, expiry)
            .map_or_else(
                || {
                    Err(StdError::generic_err(format!(
                        "Pyth quote is older than {expiry} seconds, please update"
                    )))
                },
                |price| {
                    Ok(Uint128::new(
                        // convert pricing to a base 10^18 representation for precision
                        price.price as u128 * 10_u128.pow(18) / 10_u128.pow(exponent),
                    ))
                },
            )
    }
}

pub struct MockPriceQuerier;

impl PriceQuerier for MockPriceQuerier {
    fn query_asset_price(
        &self,
        _querier: &QuerierWrapper,
        _contract_address: Addr,
        identifier: PriceIdentifier,
        _time: i64,
        _expiry: u64,
        exponent: u32,
    ) -> StdResult<Uint128> {
        if identifier
            == PriceIdentifier::from_hex(
                "5867f5683c757393a0670ef0f701490950fe93fdb006d181c8265a831ac0c5c6",
            )
            .unwrap()
        {
            return Ok(Uint128::new(
                164_243_925_u128 * 10_u128.pow(18) / 10_u128.pow(exponent),
            ));
        }
        if identifier
            == PriceIdentifier::from_hex(
                "b00b60f88b03a6a625a8d1c048c3f66653edf217439983d037e7222c4e612819",
            )
            .unwrap()
        {
            return Ok(Uint128::new(
                1_031_081_328_u128 * 10_u128.pow(18) / 10_u128.pow(exponent),
            ));
        }
        if identifier
            == PriceIdentifier::from_hex(
                "ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace",
            )
            .unwrap()
        {
            return Ok(Uint128::new(
                278_558_964_008_u128 * 10_u128.pow(18) / 10_u128.pow(exponent),
            ));
        }

        Ok(Uint128::zero())
    }
}
