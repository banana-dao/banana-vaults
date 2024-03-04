use crate::{
    error::ContractError,
    msg::{
        ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg, Swap, TotalAssetsResponse,
        VaultParticipant, VaultParticipantsResponse, WhitelistedDepositorsResponse,
    },
    state::{
        Config, ACCOUNTS_PENDING_ACTIVATION, ADDRESSES_WAITING_FOR_EXIT, ASSETS_PENDING_ACTIVATION,
        CAP_REACHED, CONFIG, HALT_EXITS_AND_JOINS, LAST_UPDATE, NON_VAULT_REWARDS, VAULT_RATIO,
        VAULT_TERMINATED, WHITELISTED_DEPOSITORS,
    },
};
use cosmwasm_std::{
    attr, coin, entry_point, to_json_binary, Addr, Attribute, BankMsg, Binary, Coin, Coins,
    CosmosMsg, Decimal, Deps, DepsMut, Empty, Env, MessageInfo, Order, QuerierWrapper, Response,
    StdError, StdResult, Storage, Uint128, WasmMsg,
};
use cw2::{get_contract_version, set_contract_version};
use cw_ownable::{assert_owner, get_ownership, initialize_owner, update_ownership, Action};
use cw_storage_plus::Bound;
use osmosis_std_modified::types::osmosis::{
    concentratedliquidity::v1beta1::{
        ConcentratedliquidityQuerier, FullPositionBreakdown, MsgAddToPosition,
        MsgCollectIncentives, MsgCollectSpreadRewards, MsgCreatePosition, Pool,
        UserPositionsResponse,
    },
    poolmanager::v1beta1::{MsgSplitRouteSwapExactAmountIn, PoolmanagerQuerier},
};
use osmosis_std_modified::types::{
    cosmos::base::v1beta1::Coin as CosmosCoin,
    osmosis::concentratedliquidity::v1beta1::MsgWithdrawPosition,
};
use pyth_sdk_cw::{query_price_feed, PriceIdentifier};
use std::{collections::HashMap, ops::Mul, str::FromStr};

// version info for migration info
const CONTRACT_NAME: &str = env!("CARGO_PKG_NAME");
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const PYTH_TESTNET_CONTRACT_ADDRESS: &str =
    "osmo1hpdzqku55lmfmptpyj6wdlugqs5etr6teqf7r4yqjjrxjznjhtuqqu5kdh";
const PYTH_MAINNET_CONTRACT_ADDRESS: &str =
    "osmo13ge29x4e2s63a8ytz2px8gurtyznmue4a69n5275692v3qn3ks8q7cwck7";

// Sensible defaults for update frequency
const DEFAULT_MIN_UPDATE_FREQUENCY: u64 = 600; // 10 minutes
const DEFAULT_MAX_UPDATE_FREQUENCY: u64 = 86400 * 14; // 14 days

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

    // validate and set the operator address
    deps.api.addr_validate(msg.operator.as_str())?;
    initialize_owner(deps.storage, deps.api, Some(msg.operator.as_str()))?;

    let pm_querier = PoolmanagerQuerier::new(&deps.querier);

    let pool_response = pm_querier.pool(msg.pool_id)?;

    let pool: Pool = match pool_response.pool {
        Some(pool) => {
            if pool
                .type_url
                .ne(&"/osmosis.concentratedliquidity.v1beta1.Pool".to_string())
            {
                return Err(ContractError::PoolIsNotCL {});
            }
            prost::Message::decode(pool.value.as_slice()).unwrap()
        }
        None => {
            return Err(ContractError::PoolNotFound {
                pool_id: msg.pool_id,
            });
        }
    };

    let pyth_contract_address = if msg.mainnet.unwrap_or(true) {
        PYTH_MAINNET_CONTRACT_ADDRESS
    } else {
        PYTH_TESTNET_CONTRACT_ADDRESS
    };

    let config = Config {
        name: msg.name,
        description: msg.description,
        image: msg.image,
        pool_id: msg.pool_id,
        asset0: msg.asset0,
        asset1: msg.asset1,
        min_uptime: msg.min_uptime,
        dollar_cap: msg.dollar_cap,
        pyth_contract_address: Addr::unchecked(pyth_contract_address),
        price_expiry: msg.price_expiry,
        min_update_frequency: msg
            .min_update_frequency
            .unwrap_or(DEFAULT_MIN_UPDATE_FREQUENCY),
        max_update_frequency: msg
            .max_update_frequency
            .unwrap_or(DEFAULT_MAX_UPDATE_FREQUENCY),
        commission: msg.commission,
        commission_receiver: msg.commission_receiver.unwrap_or(info.sender.to_owned()),
    };

    // Check that the assets in the pool are the same assets we sent in the instantiate message
    verify_config(&config, pool)?;

    // Check that funds sent match with config
    verify_funds(&info, &config)?;

    // Check that funds sent are above minimum deposit
    verify_deposit_minimum(&info, &config)?;

    CONFIG.save(deps.storage, &config)?;
    ASSETS_PENDING_ACTIVATION.save(
        deps.storage,
        &vec![coin(0, config.asset0.denom), coin(0, config.asset1.denom)],
    )?;
    ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &vec![])?;

    // At the beginning, the instantiator owns 100% of the vault
    VAULT_RATIO.save(deps.storage, info.sender.to_owned(), &Decimal::one())?;

    // Set current block time as last update
    LAST_UPDATE.save(deps.storage, &env.block.time.seconds())?;

    CAP_REACHED.save(deps.storage, &false)?;
    HALT_EXITS_AND_JOINS.save(deps.storage, &false)?;
    VAULT_TERMINATED.save(deps.storage, &false)?;
    NON_VAULT_REWARDS.save(deps.storage, &vec![])?;

    Ok(Response::new()
        .add_attribute("action", "banana_vault_instantiate")
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
        ExecuteMsg::UpdateOwnership(action) => execute_update_ownership(deps, env, info, action),
        ExecuteMsg::ModifyConfig { config } => execute_modify_config(deps, info, *config),
        ExecuteMsg::Whitelist { add, remove } => execute_whitelist(deps, info, add, remove),
        ExecuteMsg::Join {} => execute_join(deps, info),
        ExecuteMsg::Leave { address } => execute_leave(deps, info, address),
        ExecuteMsg::CreatePosition {
            lower_tick,
            upper_tick,
            tokens_provided,
            token_min_amount0,
            token_min_amount1,
            swap,
        } => execute_create_position(
            deps,
            env,
            info,
            lower_tick,
            upper_tick,
            &tokens_provided,
            token_min_amount0,
            token_min_amount1,
            swap,
        ),
        ExecuteMsg::AddToPosition {
            position_id,
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
            swap,
        } => execute_add_to_position(
            deps,
            env,
            info,
            position_id,
            amount0,
            amount1,
            token_min_amount0,
            token_min_amount1,
            swap,
        ),
        ExecuteMsg::WithdrawPosition {
            position_id,
            liquidity_amount,
        } => execute_withdraw_position(deps, env, info, position_id, liquidity_amount),
        ExecuteMsg::ProcessNewEntriesAndExits {} => {
            execute_process_new_entries_and_exits(deps, env, info)
        }
        ExecuteMsg::Halt {} => execute_halt(deps, info),
        ExecuteMsg::Resume {} => execute_resume(deps, info),
        ExecuteMsg::CloseVault {} => execute_close_vault(deps, info),
        ExecuteMsg::ForceExits {} => execute_force_exits(deps, env),
    }
}

fn execute_update_ownership(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    action: Action,
) -> Result<Response, ContractError> {
    let ownership = update_ownership(deps, &env.block, &info.sender, action)?;
    Ok(Response::new().add_attributes(ownership.into_attributes()))
}

fn execute_modify_config(
    deps: DepsMut,
    info: MessageInfo,
    new_config: Config,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;

    deps.api
        .addr_validate(new_config.pyth_contract_address.as_str())?;

    let old_config = CONFIG.load(deps.storage)?;

    if new_config.asset0 != old_config.asset0 || new_config.asset1 != old_config.asset1 {
        return Err(ContractError::CannotChangeAssets {});
    }

    if new_config.pool_id != old_config.pool_id {
        return Err(ContractError::CannotChangePoolId {});
    }

    CONFIG.save(deps.storage, &new_config)?;

    Ok(Response::new()
        .add_attribute("action", "banana_vault_modify_config")
        .add_attribute("new_pyth_address", new_config.pyth_contract_address))
}

fn execute_whitelist(
    deps: DepsMut,
    info: MessageInfo,
    add: Vec<Addr>,
    remove: Vec<Addr>,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let mut attributes: Vec<Attribute> = vec![];
    for address in add {
        if (WHITELISTED_DEPOSITORS.may_load(deps.storage, info.sender.to_owned())?).is_some() {
            return Err(ContractError::AddressInWhitelist {
                address: address.to_string(),
            });
        }

        deps.api.addr_validate(address.as_str())?;
        WHITELISTED_DEPOSITORS.save(deps.storage, info.sender.to_owned(), &Empty {})?;
        attributes.push(attr("action", "banana_vault_whitelist_add"));
        attributes.push(attr("address", address));
    }

    for address in remove {
        match WHITELISTED_DEPOSITORS.may_load(deps.storage, info.sender.to_owned())? {
            Some(_) => {
                WHITELISTED_DEPOSITORS.remove(deps.storage, info.sender.to_owned());
                attributes.push(attr("action", "banana_vault_whitelist_remove"));
                attributes.push(attr("address", address))
            }
            None => {
                return Err(ContractError::AddressNotInWhitelist {
                    address: address.to_string(),
                });
            }
        }
    }

    Ok(Response::new().add_attributes(attributes))
}

fn execute_join(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    verify_funds(&info, &config)?;
    verify_deposit_minimum(&info, &config)?;

    // Check if vault is closed
    if VAULT_TERMINATED.load(deps.storage)? {
        return Err(ContractError::VaultClosed {});
    }

    // Check if vault is halted or cap has been reached
    if HALT_EXITS_AND_JOINS.load(deps.storage)? {
        return Err(ContractError::VaultHalted {});
    }

    // Check if vault cap has been reached and user is not whitelisted to exceed it
    if CAP_REACHED.load(deps.storage)?
        && WHITELISTED_DEPOSITORS
            .may_load(deps.storage, info.sender.to_owned())?
            .is_none()
    {
        return Err(ContractError::CapReached {});
    }

    // Check if user is already waiting to exit
    if ADDRESSES_WAITING_FOR_EXIT
        .load(deps.storage)?
        .contains(&info.sender)
    {
        return Err(ContractError::AddressPendingExit {
            address: info.sender.to_string(),
        });
    }

    // We queue up the assets for the next iteration
    let mut assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

    for asset in info.funds.iter() {
        if asset.denom == config.asset0.denom {
            assets_pending[0].amount += asset.amount;
        } else if asset.denom == config.asset1.denom {
            assets_pending[1].amount += asset.amount;
        } else {
            return Err(ContractError::InvalidFunds {});
        }
    }

    ASSETS_PENDING_ACTIVATION.save(deps.storage, &assets_pending)?;

    // Check if user added to the current pending amount or if it's the first time he added

    if let Some(mut funds) =
        ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, info.sender.to_owned())?
    {
        for fund in &info.funds {
            if fund.denom == config.asset0.denom {
                funds[0].amount += fund.amount;
            } else if fund.denom == config.asset1.denom {
                funds[1].amount += fund.amount;
            }
        }
        ACCOUNTS_PENDING_ACTIVATION.save(deps.storage, info.sender, &funds)?;
    } else {
        let mut amounts_to_add = vec![
            coin(0, config.asset0.denom.to_owned()),
            coin(0, config.asset1.denom.to_owned()),
        ];

        for fund in info.funds.iter() {
            if fund.denom == config.asset0.denom {
                amounts_to_add[0].amount += fund.amount;
            } else if fund.denom == config.asset1.denom {
                amounts_to_add[1].amount += fund.amount;
            }
        }
        ACCOUNTS_PENDING_ACTIVATION.save(deps.storage, info.sender, &amounts_to_add)?;
    }

    Ok(Response::new().add_attribute("action", "banana_vault_join"))
}

fn execute_leave(
    deps: DepsMut,
    info: MessageInfo,
    address: Option<Addr>,
) -> Result<Response, ContractError> {
    let mut attributes: Vec<Attribute> = vec![attr("action", "banana_vault_leave")];

    let leave_address = match address {
        Some(address) => {
            match assert_owner(deps.storage, &info.sender) {
                Ok(_) => (),
                Err(_) => {
                    return Err(ContractError::CannotForceExit {});
                }
            }
            attributes.push(attr("action", "banana_vault_force_exit"));
            address
        }
        None => info.sender,
    };

    attributes.push(attr("address", leave_address.to_owned()));

    // If a user is leaving, we return any pending joining assets and add him in the list for leaving the vault if he has active assets in it
    let mut response = Response::new().add_attributes(attributes);

    // Check if vault is not halted for security reasons
    if HALT_EXITS_AND_JOINS.load(deps.storage)? {
        return Err(ContractError::VaultHalted {});
    }

    if let Some(mut funds) =
        ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, leave_address.to_owned())?
    {
        // We return the pending joining assets
        let mut assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;
        assets_pending[0].amount -= funds[0].amount;
        assets_pending[1].amount -= funds[1].amount;
        ASSETS_PENDING_ACTIVATION.save(deps.storage, &assets_pending)?;

        ACCOUNTS_PENDING_ACTIVATION.remove(deps.storage, leave_address.to_owned());

        // Remove empty amounts to avoid sending empty funds in bank msg
        funds.retain(|f| f.amount.ne(&Uint128::zero()));

        let send_msg = BankMsg::Send {
            to_address: leave_address.to_string(),
            amount: funds,
        };

        response = response.add_message(send_msg);
    }

    if VAULT_RATIO.has(deps.storage, leave_address.to_owned()) {
        // We add the user to the list of addresses waiting to leave the vault
        let mut addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;
        if !addresses_waiting_for_exit.contains(&leave_address) {
            addresses_waiting_for_exit.push(leave_address);
            ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &addresses_waiting_for_exit)?;
        }
    }

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
fn execute_create_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    lower_tick: i64,
    upper_tick: i64,
    tokens_provided: &Vec<Coin>,
    token_min_amount0: String,
    token_min_amount1: String,
    swap: Option<Swap>,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;

    // limit number of open positions to 10 for safety as we don't need more and don't want to bother with pagination
    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
    if cl_querier
        .user_positions(env.contract.address.to_string(), config.pool_id, None)?
        .positions
        .len()
        >= 10
    {
        return Err(ContractError::MaxPositionsReached {});
    }

    let mut messages = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    let mut balance_asset0 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset0.denom.to_owned(),
    )?;
    let mut balance_asset1 = deps.querier.query_balance(
        env.contract.address.to_owned(),
        config.asset1.denom.to_owned(),
    )?;

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
            pool_id: config.pool_id,
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

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

#[allow(clippy::too_many_arguments)]
fn execute_add_to_position(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    position_id: u64,
    amount0: String,
    amount1: String,
    token_min_amount0: String,
    token_min_amount1: String,
    swap: Option<Swap>,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;

    // since add to position actually creates a new position
    ensure_uptime(&deps, &env, position_id)?;

    let config = CONFIG.load(deps.storage)?;

    let contract_address = env.contract.address;
    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    let mut balance_asset0 = deps
        .querier
        .query_balance(contract_address.clone(), config.asset0.denom.to_owned())?;
    let mut balance_asset1 = deps
        .querier
        .query_balance(contract_address.clone(), config.asset1.denom.to_owned())?;

    // Collect rewards instead of letting them be claimed when adding to position
    let rewards = collect_rewards(&deps, contract_address.to_string(), position_id)?;
    NON_VAULT_REWARDS.save(deps.storage, &rewards.non_vault)?;
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
        coin(amount0.parse::<u128>()?, config.asset0.denom),
        coin(amount1.parse::<u128>()?, config.asset1.denom),
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
    env: Env,
    info: MessageInfo,
    position_id: u64,
    liquidity_amount: String,
) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    ensure_uptime(&deps, &env, position_id)?;

    let config = CONFIG.load(deps.storage)?;

    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    // Collect all rewards first to claim commission and distribute the rest
    let rewards = collect_rewards(&deps, env.contract.address.to_string(), position_id)?;
    NON_VAULT_REWARDS.save(deps.storage, &rewards.non_vault)?;
    messages.extend(rewards.messages);
    attributes.extend(rewards.attributes);

    let msg_withdraw_position: CosmosMsg = MsgWithdrawPosition {
        position_id,
        sender: env.contract.address.to_string(),
        liquidity_amount,
    }
    .into();

    messages.push(msg_withdraw_position);
    attributes.push(attr("action", "banana_vault_withdraw_position"));

    let last_update = LAST_UPDATE.load(deps.storage)?;

    // We will only execute this message if it's time for update (it will execute after the withdraw and check if there are any positions open)
    let update_users_msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: env.contract.address.to_string(),
        msg: to_json_binary(&ExecuteMsg::ProcessNewEntriesAndExits {})?,
        funds: vec![],
    });

    // We can only update if it's time for update and if vault is not halted
    if !HALT_EXITS_AND_JOINS.load(deps.storage)?
        && env.block.time.seconds() >= last_update + config.min_update_frequency
    {
        messages.push(update_users_msg)
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(attributes))
}

struct Rewards {
    amount0: Uint128,
    amount1: Uint128,
    non_vault: Vec<Coin>,
    messages: Vec<CosmosMsg>,
    attributes: Vec<Attribute>,
}

// This can only be done by contract internally
fn execute_process_new_entries_and_exits(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
) -> Result<Response, ContractError> {
    if info.sender != env.contract.address {
        return Err(ContractError::Unauthorized {});
    }

    let config = CONFIG.load(deps.storage)?;

    // can only proceed if no current positions are open
    if ConcentratedliquidityQuerier::new(&deps.querier)
        .user_positions(env.contract.address.to_string(), config.pool_id, None)?
        .positions
        .is_empty()
    {
        Ok(Response::new()
            .add_messages(process_entries_and_exits(deps, env)?)
            .add_attribute("action", "banana_vault_process_new_entries_and_exits"))
    } else {
        Ok(Response::new())
    }
}

fn execute_halt(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    HALT_EXITS_AND_JOINS.save(deps.storage, &true)?;
    Ok(Response::new().add_attribute("action", "banana_vault_halt"))
}

fn execute_resume(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    HALT_EXITS_AND_JOINS.save(deps.storage, &false)?;
    Ok(Response::new().add_attribute("action", "banana_vault_resume"))
}

fn execute_close_vault(deps: DepsMut, info: MessageInfo) -> Result<Response, ContractError> {
    assert_owner(deps.storage, &info.sender)?;
    let config = CONFIG.load(deps.storage)?;

    let mut messages = vec![];

    // We get all addresses that are waiting to join and send the funds back

    let addresses_pending_activation: Vec<(Addr, Vec<Coin>)> = ACCOUNTS_PENDING_ACTIVATION
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(Result::ok)
        .collect();

    for each_address in &addresses_pending_activation {
        let mut funds = each_address.1.to_owned();

        // Remove empty amounts to avoid sending empty funds in bank msg
        funds.retain(|f| f.amount.ne(&Uint128::zero()));

        let send_msg = BankMsg::Send {
            to_address: each_address.0.to_string(),
            amount: funds,
        };

        messages.push(send_msg);
    }

    // We get all addresses that are waiting to exit and add all the ones that are in the vault and are not waiting for exit
    let mut addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;
    let ratios: Vec<(Addr, Decimal)> = VAULT_RATIO
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(|v| v.ok())
        .collect();

    for ratio in ratios.iter() {
        if !addresses_waiting_for_exit.contains(&ratio.0) {
            addresses_waiting_for_exit.push(ratio.0.to_owned());
        }
    }

    // Save all in state
    // No one else is waiting to join (we sent all funds back)
    // No funds are waiting to join
    // Everyone is added to exit list
    // Vault is terminated (no one can join anymore)

    ACCOUNTS_PENDING_ACTIVATION.clear(deps.storage);
    ASSETS_PENDING_ACTIVATION.save(
        deps.storage,
        &vec![coin(0, config.asset0.denom), coin(0, config.asset1.denom)],
    )?;
    ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &addresses_waiting_for_exit)?;
    VAULT_TERMINATED.save(deps.storage, &true)?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "banana_vault_terminate"))
}

fn execute_force_exits(deps: DepsMut, env: Env) -> Result<Response, ContractError> {
    let last_update = LAST_UPDATE.load(deps.storage)?;
    let config = CONFIG.load(deps.storage)?;

    if env.block.time.seconds() < last_update + config.max_update_frequency {
        return Err(ContractError::CantForceExitsYet {
            seconds: last_update + config.max_update_frequency - env.block.time.seconds(),
        });
    }

    let mut messages = vec![];
    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
    let user_positions_response: UserPositionsResponse =
        cl_querier.user_positions(env.contract.address.to_string(), config.pool_id, None)?;

    for position in user_positions_response.positions.iter() {
        let msg_withdraw_position: CosmosMsg = MsgWithdrawPosition {
            position_id: position.position.as_ref().unwrap().position_id,
            sender: env.contract.address.to_string(),
            liquidity_amount: position.position.as_ref().unwrap().liquidity.to_owned(),
        }
        .into();

        messages.push(msg_withdraw_position);
    }

    let update_users_msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: env.contract.address.to_string(),
        msg: to_json_binary(&ExecuteMsg::ProcessNewEntriesAndExits {})?,
        funds: vec![],
    });

    messages.push(update_users_msg);

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "banana_vault_force_exits"))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&query_config(deps)?),
        QueryMsg::Ownership {} => to_json_binary(&get_ownership(deps.storage)?),
        QueryMsg::TotalActiveAssets {} => to_json_binary(&query_total_active_assets(deps, env)?),
        QueryMsg::TotalPendingAssets {} => to_json_binary(&query_total_pending_assets(deps)?),
        QueryMsg::CanUpdate {} => to_json_binary(&query_can_update(deps, env)?),
        QueryMsg::PendingJoin { address } => to_json_binary(&query_pending_join(deps, address)?),
        QueryMsg::AccountsPendingExit {} => to_json_binary(&query_pending_exits(deps)?),
        QueryMsg::VaultRatio { address } => to_json_binary(&query_vault_ratio(deps, address)?),
        QueryMsg::WhitelistedDepositors { start_after, limit } => {
            to_json_binary(&query_whitelisted_depositors(deps, start_after, limit))
        }
        QueryMsg::VaultParticipants { start_after, limit } => {
            to_json_binary(&query_vault_participants(deps, start_after, limit))
        }
    }
}

fn query_config(deps: Deps) -> StdResult<Config> {
    let config = CONFIG.load(deps.storage)?;
    Ok(config)
}

fn query_total_active_assets(deps: Deps, env: Env) -> StdResult<TotalAssetsResponse> {
    let config = CONFIG.load(deps.storage)?;
    let address = env.contract.address.to_string();
    let commission_remainder = Decimal::one() - config.commission.unwrap_or(Decimal::zero());

    let (mut asset0, mut asset1) = get_available_balances(&deps, &address)?;

    let cl_querier = ConcentratedliquidityQuerier::new(&deps.querier);
    for position in &cl_querier
        .user_positions(address.clone(), config.pool_id, None)?
        .positions
    {
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
                if incentive.denom == config.asset0.denom {
                    asset0.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                } else if incentive.denom == config.asset1.denom {
                    asset1.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                }
            }
        }

        if !position.claimable_spread_rewards.is_empty() {
            for incentive in &position.claimable_spread_rewards {
                if incentive.denom == config.asset0.denom {
                    asset0.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                } else if incentive.denom == config.asset1.denom {
                    asset1.amount +=
                        Uint128::from_str(&incentive.amount)?.mul_floor(commission_remainder);
                }
            }
        }
    }

    Ok(TotalAssetsResponse { asset0, asset1 })
}

fn query_total_pending_assets(deps: Deps) -> StdResult<TotalAssetsResponse> {
    let pending_assets = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;
    Ok(TotalAssetsResponse {
        asset0: pending_assets[0].clone(),
        asset1: pending_assets[1].clone(),
    })
}

fn query_can_update(deps: Deps, env: Env) -> StdResult<bool> {
    Ok(env.block.time.seconds()
        >= LAST_UPDATE.load(deps.storage)? + CONFIG.load(deps.storage)?.min_update_frequency)
}

fn query_pending_join(deps: Deps, address: Addr) -> StdResult<Vec<Coin>> {
    Ok(ACCOUNTS_PENDING_ACTIVATION
        .may_load(deps.storage, address)?
        .unwrap_or_default())
}

fn query_pending_exits(deps: Deps) -> StdResult<Vec<Addr>> {
    ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)
}

fn query_vault_ratio(deps: Deps, address: Addr) -> StdResult<Decimal> {
    let ratio = VAULT_RATIO
        .may_load(deps.storage, address)?
        .unwrap_or_default();
    Ok(ratio)
}

fn query_whitelisted_depositors(
    deps: Deps,
    start_after: Option<Addr>,
    limit: Option<u32>,
) -> WhitelistedDepositorsResponse {
    let limit = limit.unwrap_or(MAX_PAGE_LIMIT).min(MAX_PAGE_LIMIT);
    let start = start_after.map(Bound::exclusive);
    let whitelisted_depositors: Vec<Addr> = WHITELISTED_DEPOSITORS
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit as usize)
        .filter_map(|v| v.ok())
        .map(|(addr, _)| addr)
        .collect();

    WhitelistedDepositorsResponse {
        whitelisted_depositors,
    }
}

fn query_vault_participants(
    deps: Deps,
    start_after: Option<Addr>,
    limit: Option<u32>,
) -> VaultParticipantsResponse {
    let limit = limit.unwrap_or(MAX_PAGE_LIMIT).min(MAX_PAGE_LIMIT);
    let start = start_after.map(Bound::exclusive);
    let vault_participants: Vec<VaultParticipant> = VAULT_RATIO
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit as usize)
        .filter_map(|v| v.ok())
        .map(|(address, ratio)| VaultParticipant { address, ratio })
        .collect();

    VaultParticipantsResponse { vault_participants }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> StdResult<Response> {
    let version = get_contract_version(deps.storage)?;
    if version.contract != CONTRACT_NAME {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    }
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    Ok(Response::default())
}

// Helpers
fn verify_config(config: &Config, pool: Pool) -> Result<(), ContractError> {
    if config.asset0.denom != pool.token0 {
        return Err(ContractError::InvalidConfigAsset { asset: 0 });
    }

    if config.asset1.denom != pool.token1 {
        return Err(ContractError::InvalidConfigAsset { asset: 1 });
    }

    Ok(())
}

fn verify_funds(info: &MessageInfo, config: &Config) -> Result<(), ContractError> {
    if info.funds.is_empty() {
        return Err(ContractError::NoFunds {});
    }

    if info.funds.len() == 2 && info.funds[0].denom == info.funds[1].denom {
        return Err(ContractError::InvalidFunds {});
    }

    if info.funds.len() > 2 {
        return Err(ContractError::InvalidFunds {});
    }

    for fund in &info.funds {
        if fund.denom != config.asset0.denom && fund.denom != config.asset1.denom {
            return Err(ContractError::InvalidFunds {});
        }
    }

    Ok(())
}

fn verify_deposit_minimum(info: &MessageInfo, config: &Config) -> Result<(), ContractError> {
    for token_provided in &info.funds {
        if token_provided.denom == config.asset0.denom
            && token_provided.amount.lt(&config.asset0.min_deposit)
        {
            return Err(ContractError::DepositBelowMinimum {
                denom: config.asset0.denom.clone(),
            });
        }
        if token_provided.denom == config.asset1.denom
            && token_provided.amount.lt(&config.asset1.min_deposit)
        {
            return Err(ContractError::DepositBelowMinimum {
                denom: config.asset1.denom.clone(),
            });
        }
    }

    Ok(())
}

fn verify_availability_of_funds(
    storage: &mut dyn Storage,
    tokens_provided: &Vec<Coin>,
    amount_asset0: Uint128,
    amount_asset1: Uint128,
) -> Result<(), ContractError> {
    let assets_pending = ASSETS_PENDING_ACTIVATION.load(storage)?;
    let config = CONFIG.load(storage)?;

    for token_provided in tokens_provided {
        if token_provided.denom == config.asset0.denom
            && token_provided.amount > amount_asset0.checked_sub(assets_pending[0].amount)?
        {
            return Err(ContractError::CannotAddMoreThanAvailableForAsset {
                asset: config.asset0.denom,
                amount: amount_asset0
                    .checked_sub(assets_pending[0].amount)?
                    .to_string(),
            });
        }
        if token_provided.denom == config.asset1.denom
            && token_provided.amount > amount_asset1.checked_sub(assets_pending[1].amount)?
        {
            return Err(ContractError::CannotAddMoreThanAvailableForAsset {
                asset: config.asset1.denom,
                amount: amount_asset1
                    .checked_sub(assets_pending[1].amount)?
                    .to_string(),
            });
        }
    }

    Ok(())
}

fn ensure_uptime(deps: &DepsMut, env: &Env, position_id: u64) -> Result<(), ContractError> {
    // if uptime is set, check if enough time has passed to withdraw without forfeiting rewards
    if let Some(uptime) = CONFIG.load(deps.storage)?.min_uptime {
        let join_time: u64 = match ConcentratedliquidityQuerier::new(&deps.querier)
            .position_by_id(position_id)?
            .position
        {
            Some(position) => position.position.unwrap().join_time.unwrap().seconds as u64,
            None => {
                return Err(ContractError::NoPositionsOpen {});
            }
        };

        // add one second for rounding safety
        if env.block.time.seconds() - join_time < uptime + 1 {
            Err(ContractError::MinUptime())
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

// These are all the assets the vault has that are not pending to join
fn get_available_balances(deps: &Deps, address: &String) -> Result<(Coin, Coin), StdError> {
    let config = CONFIG.load(deps.storage)?;
    let balance_asset0 = deps
        .querier
        .query_balance(address.clone(), config.asset0.denom.to_owned())?;
    let balance_asset1 = deps
        .querier
        .query_balance(address, config.asset1.denom.to_owned())?;

    let assets_pending = ASSETS_PENDING_ACTIVATION.load(deps.storage)?;

    Ok((
        coin(
            balance_asset0
                .amount
                .checked_sub(assets_pending[0].amount)?
                .u128(),
            config.asset0.denom.to_owned(),
        ),
        coin(
            balance_asset1
                .amount
                .checked_sub(assets_pending[1].amount)?
                .u128(),
            config.asset1.denom.to_owned(),
        ),
    ))
}

// collect_rewards should be called upon any position change
fn collect_rewards(
    deps: &DepsMut,
    sender: String,
    position_id: u64,
) -> Result<Rewards, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    let position: FullPositionBreakdown = match ConcentratedliquidityQuerier::new(&deps.querier)
        .position_by_id(position_id)?
        .position
    {
        Some(position) => position,
        None => {
            return Err(ContractError::NoPositionsOpen {});
        }
    };

    let mut reward_coins: Coins = Coins::default();
    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attributes: Vec<Attribute> = vec![];

    if !position.claimable_incentives.is_empty() {
        for incentive in &position.claimable_incentives {
            reward_coins.add(coin(
                Uint128::from_str(&incentive.amount)?.u128(),
                incentive.denom.to_owned(),
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
                incentive.denom.to_owned(),
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
    let mut commission_coins: Coins = Coins::default();
    let mut non_vault_rewards: Coins =
        Coins::try_from(NON_VAULT_REWARDS.load(deps.storage)?).unwrap_or_default();

    for reward_coin in &reward_coins {
        let commission_amount = reward_coin
            .amount
            .mul_floor(config.commission.unwrap_or(Decimal::zero()));

        // might be a zero coin but it won't get added
        commission_coins.add(coin(commission_amount.u128(), reward_coin.denom.to_owned()))?;

        if reward_coin.denom == config.asset0.denom {
            amount0 = reward_coin.amount.checked_sub(commission_amount)?;
        } else if reward_coin.denom == config.asset1.denom {
            amount1 = reward_coin.amount.checked_sub(commission_amount)?;
        } else {
            // add to existing non vault rewards. we could distribute them here,
            // but we will do it in the next update to avoid potential repeated dust sends
            non_vault_rewards.add(coin(
                reward_coin.amount.checked_sub(commission_amount)?.u128(),
                reward_coin.denom.to_owned(),
            ))?;
        }
    }

    if !commission_coins.is_empty() {
        let send_msg = BankMsg::Send {
            to_address: config.commission_receiver.to_string(),
            amount: commission_coins.into(),
        };
        attributes.push(attr("action", "banana_vault_collect_commission"));
        messages.push(send_msg.into());
    }

    Ok(Rewards {
        amount0,
        amount1,
        non_vault: non_vault_rewards.into(),
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
            return Err(ContractError::CannotSwapMoreThanAvailable {});
        }
        asset0.amount = asset0.amount.checked_sub(token_in_amount)?;
    }

    if swap.token_in_denom == asset1.denom {
        if token_in_amount > asset1.amount {
            return Err(ContractError::CannotSwapMoreThanAvailable {});
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

fn process_entries_and_exits(deps: DepsMut, env: Env) -> Result<Vec<CosmosMsg>, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    let mut non_vault_rewards: Vec<Coin> = NON_VAULT_REWARDS.load(deps.storage)?;
    let mut total_non_vault_rewards = Coins::default();
    let addresses_waiting_for_exit = ADDRESSES_WAITING_FOR_EXIT.load(deps.storage)?;

    let ratios: Vec<(Addr, Decimal)> = VAULT_RATIO
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(Result::ok)
        .collect();

    let (available_asset0, available_asset1) =
        get_available_balances(&deps.as_ref(), &env.contract.address.to_string())?;

    let dec_available_asset0 = Decimal::new(available_asset0.amount);
    let dec_available_asset1 = Decimal::new(available_asset1.amount);

    let current_time = env.block.time.seconds();

    let price_querier: &dyn PriceQuerier = if config.pyth_contract_address
        == Addr::unchecked("osmo1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqmcn030")
    {
        &MockPriceQuerier
    } else {
        &PythQuerier
    };

    let current_price_asset0 = price_querier.query_asset_price(
        &deps.querier,
        config.pyth_contract_address.clone(),
        config.asset0.price_identifier,
        current_time as i64,
        config.price_expiry,
        config.asset0.decimals,
    )?;

    let current_price_asset1 = price_querier.query_asset_price(
        &deps.querier,
        config.pyth_contract_address,
        config.asset1.price_identifier,
        current_time as i64,
        config.price_expiry,
        config.asset1.decimals,
    )?;

    let mut messages: Vec<CosmosMsg> = vec![];
    let mut total_dollars_in_vault = Decimal::zero();
    let mut address_dollars_map: HashMap<Addr, Decimal> = HashMap::new();

    for (address, ratio) in &ratios {
        let mut amounts_send_msg: Vec<Coin> = vec![];

        // get the non vault rewards for each address and prepare to send
        let rewards: Vec<Coin> = non_vault_rewards
            .iter()
            .map(|c| {
                let amount = c.amount.mul_floor(*ratio);
                coin(amount.u128(), &c.denom)
            })
            .filter(|c| !c.amount.is_zero())
            .map(|c| {
                total_non_vault_rewards.add(c.clone()).unwrap();
                c
            })
            .collect();

        amounts_send_msg.extend(rewards);

        if addresses_waiting_for_exit.contains(address) {
            // if the address is waiting for exit, add the funds to to withdraw
            let amount_to_send_asset0 = available_asset0.amount.mul_floor(*ratio);
            let amount_to_send_asset1 = available_asset1.amount.mul_floor(*ratio);

            if amount_to_send_asset0.gt(&Uint128::zero()) {
                amounts_send_msg.push(coin(
                    amount_to_send_asset0.u128(),
                    config.asset0.denom.to_owned(),
                ));
            }

            if amount_to_send_asset1.gt(&Uint128::zero()) {
                amounts_send_msg.push(coin(
                    amount_to_send_asset1.u128(),
                    config.asset1.denom.to_owned(),
                ));
            }
        } else {
            // if not, collect the dollar amounts to recaulculate the vault ratios
            let mut amount_asset0 = dec_available_asset0.mul(*ratio);
            let mut amount_asset1 = dec_available_asset1.mul(*ratio);

            // If for some reason this address is actually waiting to join with more assets, we will add this here too
            if let Some(funds) =
                ACCOUNTS_PENDING_ACTIVATION.may_load(deps.storage, address.clone())?
            {
                amount_asset0 += Decimal::new(funds[0].amount);
                amount_asset1 += Decimal::new(funds[1].amount);
                // Remove it from here to avoid processing it again later
                ACCOUNTS_PENDING_ACTIVATION.remove(deps.storage, address.clone());
            }

            let dollars_asset0 = current_price_asset0.checked_mul(amount_asset0)?;
            let dollars_asset1 = current_price_asset1.checked_mul(amount_asset1)?;

            let total_amount_dollars_for_address = dollars_asset0.checked_add(dollars_asset1)?;

            total_dollars_in_vault =
                total_dollars_in_vault.checked_add(total_amount_dollars_for_address)?;

            address_dollars_map.insert(address.clone(), total_amount_dollars_for_address);
        }

        if !amounts_send_msg.is_empty() {
            messages.push(
                BankMsg::Send {
                    to_address: address.to_string(),
                    amount: amounts_send_msg,
                }
                .into(),
            );
        }
    }

    // Reduce all non vault rewards by the total amount sent
    for each_non_vault_reward in non_vault_rewards.iter_mut() {
        each_non_vault_reward.amount = each_non_vault_reward.amount.checked_sub(
            total_non_vault_rewards
                .iter()
                .find(|c| c.denom == each_non_vault_reward.denom)
                .map(|c| c.amount)
                .unwrap_or_default(),
        )?;
    }

    non_vault_rewards.retain(|c| !c.amount.is_zero());

    // If the vault is closed we send the dust left to the owner account
    if VAULT_TERMINATED.load(deps.storage)? {
        messages.push(
            BankMsg::Send {
                to_address: get_ownership(deps.storage)?.owner.unwrap().to_string(),
                amount: non_vault_rewards.clone(),
            }
            .into(),
        );
        non_vault_rewards = vec![];
    }

    NON_VAULT_REWARDS.save(deps.storage, &non_vault_rewards)?;

    // Now we are going to process all the new entries
    let new_entries: Vec<(Addr, Vec<Coin>)> = ACCOUNTS_PENDING_ACTIVATION
        .range(deps.storage, None, None, Order::Ascending)
        .filter_map(Result::ok)
        .collect();

    for new_entry in &new_entries {
        let dollars_asset0 =
            current_price_asset0.checked_mul(Decimal::new(new_entry.1[0].amount))?;
        let dollars_asset1 =
            current_price_asset1.checked_mul(Decimal::new(new_entry.1[1].amount))?;

        let total_amount_dollars_for_address = dollars_asset0.checked_add(dollars_asset1)?;

        total_dollars_in_vault =
            total_dollars_in_vault.checked_add(total_amount_dollars_for_address)?;

        address_dollars_map.insert(new_entry.0.to_owned(), total_amount_dollars_for_address);
    }

    // // remove/reset all addresses waiting for entry and exit and the ratio
    ADDRESSES_WAITING_FOR_EXIT.save(deps.storage, &vec![])?;
    ACCOUNTS_PENDING_ACTIVATION.clear(deps.storage);
    ASSETS_PENDING_ACTIVATION.save(
        deps.storage,
        &vec![coin(0, config.asset0.denom), coin(0, config.asset1.denom)],
    )?;
    VAULT_RATIO.clear(deps.storage);

    // Recalculate ratios
    for each_address in &address_dollars_map {
        let ratio = each_address.1.checked_div(total_dollars_in_vault)?;
        VAULT_RATIO.save(deps.storage, each_address.0.to_owned(), &ratio)?;
    }

    // Save last update to current block time
    LAST_UPDATE.save(deps.storage, &current_time)?;

    // Check that we are not over the vault cap, if that's the case, we will flag it to halt joins until under cap again
    if let Some(dollar_cap) = config.dollar_cap {
        CAP_REACHED.save(
            deps.storage,
            &(total_dollars_in_vault >= Decimal::new(dollar_cap)),
        )?;
    }

    Ok(messages)
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
    ) -> Result<Decimal, ContractError>;
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
    ) -> Result<Decimal, ContractError> {
        match query_price_feed(querier, contract_address, identifier)?
            .price_feed
            .get_price_no_older_than(time, expiry)
        {
            Some(price) => Ok(Decimal::from_ratio(
                price.price as u128,
                10_u64.pow(exponent),
            )),
            None => Err(ContractError::StalePrice { seconds: expiry }),
        }
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
    ) -> Result<Decimal, ContractError> {
        if identifier
            == PriceIdentifier::from_hex(
                "5867f5683c757393a0670ef0f701490950fe93fdb006d181c8265a831ac0c5c6",
            )
            .unwrap()
        {
            return Ok(Decimal::from_ratio(164243925_u128, 10_u64.pow(exponent)));
        }
        if identifier
            == PriceIdentifier::from_hex(
                "b00b60f88b03a6a625a8d1c048c3f66653edf217439983d037e7222c4e612819",
            )
            .unwrap()
        {
            return Ok(Decimal::from_ratio(1031081328_u128, 10_u64.pow(exponent)));
        }
        if identifier
            == PriceIdentifier::from_hex(
                "ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace",
            )
            .unwrap()
        {
            return Ok(Decimal::from_ratio(278558964008_u128, 10_u64.pow(18)));
        }

        Ok(Decimal::zero())
    }
}
