use osmosis_std_derive::CosmwasmExt;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
#[derive(::serde::Serialize, ::serde::Deserialize, ::schemars::JsonSchema)]
pub enum Downtime {
    Duration30s = 0,
    Duration1m = 1,
    Duration2m = 2,
    Duration3m = 3,
    Duration4m = 4,
    Duration5m = 5,
    Duration10m = 6,
    Duration20m = 7,
    Duration30m = 8,
    Duration40m = 9,
    Duration50m = 10,
    Duration1h = 11,
    Duration15h = 12,
    Duration2h = 13,
    Duration25h = 14,
    Duration3h = 15,
    Duration4h = 16,
    Duration5h = 17,
    Duration6h = 18,
    Duration9h = 19,
    Duration12h = 20,
    Duration18h = 21,
    Duration24h = 22,
    Duration36h = 23,
    Duration48h = 24,
}
impl Downtime {
    /// String value of the enum field names used in the ProtoBuf definition.
    ///
    /// The values are not transformed in any way and thus are considered stable
    /// (if the ProtoBuf definition does not change) and safe for programmatic use.
    pub fn as_str_name(&self) -> &'static str {
        match self {
            Downtime::Duration30s => "DURATION_30S",
            Downtime::Duration1m => "DURATION_1M",
            Downtime::Duration2m => "DURATION_2M",
            Downtime::Duration3m => "DURATION_3M",
            Downtime::Duration4m => "DURATION_4M",
            Downtime::Duration5m => "DURATION_5M",
            Downtime::Duration10m => "DURATION_10M",
            Downtime::Duration20m => "DURATION_20M",
            Downtime::Duration30m => "DURATION_30M",
            Downtime::Duration40m => "DURATION_40M",
            Downtime::Duration50m => "DURATION_50M",
            Downtime::Duration1h => "DURATION_1H",
            Downtime::Duration15h => "DURATION_1_5H",
            Downtime::Duration2h => "DURATION_2H",
            Downtime::Duration25h => "DURATION_2_5H",
            Downtime::Duration3h => "DURATION_3H",
            Downtime::Duration4h => "DURATION_4H",
            Downtime::Duration5h => "DURATION_5H",
            Downtime::Duration6h => "DURATION_6H",
            Downtime::Duration9h => "DURATION_9H",
            Downtime::Duration12h => "DURATION_12H",
            Downtime::Duration18h => "DURATION_18H",
            Downtime::Duration24h => "DURATION_24H",
            Downtime::Duration36h => "DURATION_36H",
            Downtime::Duration48h => "DURATION_48H",
        }
    }
    /// Creates an enum from field names used in the ProtoBuf definition.
    pub fn from_str_name(value: &str) -> ::core::option::Option<Self> {
        match value {
            "DURATION_30S" => Some(Self::Duration30s),
            "DURATION_1M" => Some(Self::Duration1m),
            "DURATION_2M" => Some(Self::Duration2m),
            "DURATION_3M" => Some(Self::Duration3m),
            "DURATION_4M" => Some(Self::Duration4m),
            "DURATION_5M" => Some(Self::Duration5m),
            "DURATION_10M" => Some(Self::Duration10m),
            "DURATION_20M" => Some(Self::Duration20m),
            "DURATION_30M" => Some(Self::Duration30m),
            "DURATION_40M" => Some(Self::Duration40m),
            "DURATION_50M" => Some(Self::Duration50m),
            "DURATION_1H" => Some(Self::Duration1h),
            "DURATION_1_5H" => Some(Self::Duration15h),
            "DURATION_2H" => Some(Self::Duration2h),
            "DURATION_2_5H" => Some(Self::Duration25h),
            "DURATION_3H" => Some(Self::Duration3h),
            "DURATION_4H" => Some(Self::Duration4h),
            "DURATION_5H" => Some(Self::Duration5h),
            "DURATION_6H" => Some(Self::Duration6h),
            "DURATION_9H" => Some(Self::Duration9h),
            "DURATION_12H" => Some(Self::Duration12h),
            "DURATION_18H" => Some(Self::Duration18h),
            "DURATION_24H" => Some(Self::Duration24h),
            "DURATION_36H" => Some(Self::Duration36h),
            "DURATION_48H" => Some(Self::Duration48h),
            _ => None,
        }
    }
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(
    Clone,
    PartialEq,
    Eq,
    ::prost::Message,
    ::serde::Serialize,
    ::serde::Deserialize,
    ::schemars::JsonSchema,
    CosmwasmExt,
)]
#[proto_message(type_url = "/osmosis.downtimedetector.v1beta1.GenesisDowntimeEntry")]
pub struct GenesisDowntimeEntry {
    #[prost(enumeration = "Downtime", tag = "1")]
    #[serde(
        serialize_with = "crate::serde::as_str::serialize",
        deserialize_with = "crate::serde::as_str::deserialize"
    )]
    pub duration: i32,
    #[prost(message, optional, tag = "2")]
    pub last_downtime: ::core::option::Option<crate::shim::Timestamp>,
}
/// GenesisState defines the twap module's genesis state.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(
    Clone,
    PartialEq,
    Eq,
    ::prost::Message,
    ::serde::Serialize,
    ::serde::Deserialize,
    ::schemars::JsonSchema,
    CosmwasmExt,
)]
#[proto_message(type_url = "/osmosis.downtimedetector.v1beta1.GenesisState")]
pub struct GenesisState {
    #[prost(message, repeated, tag = "1")]
    pub downtimes: ::prost::alloc::vec::Vec<GenesisDowntimeEntry>,
    #[prost(message, optional, tag = "2")]
    pub last_block_time: ::core::option::Option<crate::shim::Timestamp>,
}
/// Query for has it been at least $RECOVERY_DURATION units of time,
/// since the chain has been down for $DOWNTIME_DURATION.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(
    Clone,
    PartialEq,
    Eq,
    ::prost::Message,
    ::serde::Serialize,
    ::serde::Deserialize,
    ::schemars::JsonSchema,
    CosmwasmExt,
)]
#[proto_message(
    type_url = "/osmosis.downtimedetector.v1beta1.RecoveredSinceDowntimeOfLengthRequest"
)]
#[proto_query(
    path = "/osmosis.downtimedetector.v1beta1.Query/RecoveredSinceDowntimeOfLength",
    response_type = RecoveredSinceDowntimeOfLengthResponse
)]
pub struct RecoveredSinceDowntimeOfLengthRequest {
    #[prost(enumeration = "Downtime", tag = "1")]
    #[serde(
        serialize_with = "crate::serde::as_str::serialize",
        deserialize_with = "crate::serde::as_str::deserialize"
    )]
    pub downtime: i32,
    #[prost(message, optional, tag = "2")]
    pub recovery: ::core::option::Option<crate::shim::Duration>,
}
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(
    Clone,
    PartialEq,
    Eq,
    ::prost::Message,
    ::serde::Serialize,
    ::serde::Deserialize,
    ::schemars::JsonSchema,
    CosmwasmExt,
)]
#[proto_message(
    type_url = "/osmosis.downtimedetector.v1beta1.RecoveredSinceDowntimeOfLengthResponse"
)]
pub struct RecoveredSinceDowntimeOfLengthResponse {
    #[prost(bool, tag = "1")]
    pub succesfully_recovered: bool,
}
pub struct DowntimedetectorQuerier<'a, Q: cosmwasm_std::CustomQuery> {
    querier: &'a cosmwasm_std::QuerierWrapper<'a, Q>,
}
impl<'a, Q: cosmwasm_std::CustomQuery> DowntimedetectorQuerier<'a, Q> {
    pub fn new(querier: &'a cosmwasm_std::QuerierWrapper<'a, Q>) -> Self {
        Self { querier }
    }
    pub fn recovered_since_downtime_of_length(
        &self,
        downtime: i32,
        recovery: ::core::option::Option<crate::shim::Duration>,
    ) -> Result<RecoveredSinceDowntimeOfLengthResponse, cosmwasm_std::StdError> {
        RecoveredSinceDowntimeOfLengthRequest { downtime, recovery }.query(self.querier)
    }
}
