//! Validated configuration for the `web_fetch` tool's security policy.
//!
//! [`FetchConfig`] is an opaque, immutable policy value: its fields are private
//! validated newtypes, so a constructed value can never hold an invalid state.
//! Build one with [`FetchConfig::builder`] and [`FetchConfigBuilder::build`],
//! which validates every field and reports a [`ConfigError`], or take the
//! built-in safe policy with [`FetchConfig::default`].
//!
//! The policy governs what a fetch may do: the URL-policy knobs (`allow_http`,
//! `allow_ports`, `allow_ip_literals`), the address policy (denied CIDR ranges
//! and exact host-plus-address exceptions), the size caps (`max_bytes`,
//! `max_chars`), the redirect cap, the timeouts, and the `User-Agent`. The set
//! of accepted content types is fixed rather than configured.
//!
//! [`FetchConfigBuilder::build`] checks each raw builder field with a validator
//! of its own. A validator checks the field against its constraint and returns
//! the validated value the policy stores, or the private error representation
//! naming the field and the violated constraint. [`ConfigError`] wraps that
//! representation opaquely.

use std::net::IpAddr;
use std::time::Duration;

use ipnet::IpNet;
use reqwest::header::HeaderValue;

/// The default ports a fetch may target: HTTP and HTTPS.
const DEFAULT_ALLOW_PORTS: [u16; 2] = [80, 443];

/// The default cap on redirect hops a single fetch may follow.
const DEFAULT_MAX_REDIRECTS: usize = 5;

/// The hard ceiling on the redirect cap accepted by the builder.
const MAX_REDIRECTS_CEILING: usize = 20;

/// The default time allowed to establish a TCP connection.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The default cap on the total time a single request may take.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// The default time an idle connection is kept in the pool before it is closed.
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The hard ceiling on the connect timeout accepted by the builder.
const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// The hard ceiling on the whole-request timeout accepted by the builder.
const MAX_TIMEOUT: Duration = Duration::from_secs(300);

/// The hard ceiling on the pool-idle timeout accepted by the builder.
const MAX_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// The default `User-Agent` header sent on every request.
const DEFAULT_USER_AGENT: &str = "harness-webfetch/0.0";

/// The default cap on a response body's decompressed size, in bytes (8 MiB).
const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The hard ceiling on `max_bytes` accepted by the builder (64 MiB).
const MAX_BYTES_CEILING: usize = 64 * 1024 * 1024;

/// The default cap on the returned text length, in characters.
const DEFAULT_MAX_CHARS: usize = 40_000;

/// The hard ceiling on `max_chars` accepted by the builder.
const MAX_CHARS_CEILING: usize = 10_000_000;

/// A `User-Agent` string validated to be a legal HTTP header value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UserAgent(String);

/// A response-body byte cap, guaranteed in `1..=MAX_BYTES_CEILING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MaxBytes(usize);

/// A returned-text character cap, guaranteed in `1..=MAX_CHARS_CEILING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MaxChars(usize);

/// A redirect-hop cap, guaranteed in `0..=MAX_REDIRECTS_CEILING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MaxRedirects(usize);

/// A [`Duration`] guaranteed greater than `Duration::ZERO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PositiveDuration(Duration);

/// An exact host-plus-address exception, with the host canonicalized.
///
/// The host is lowercased, trimmed, and stripped of a single trailing dot, so a
/// case- or trailing-dot variant of the configured host still matches the
/// resolver's representation. Keyed on both host and address, so a rebinding
/// answer for another name cannot inherit this exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostAddressException {
    /// The canonical (lowercased, dot-stripped) host this exception names.
    host: String,
    /// The exact address the exception admits for that host.
    addr: IpAddr,
}

impl HostAddressException {
    /// Returns whether `(host, addr)` matches this exception.
    ///
    /// `host` is canonicalized the same way the entry's host was, so the
    /// comparison is case- and trailing-dot-insensitive.
    #[must_use]
    pub(crate) fn matches(&self, host: &str, addr: IpAddr) -> bool {
        self.addr == addr && self.host == canonical_host(host)
    }
}

/// Canonicalizes a DNS host for exact-exception comparison.
fn canonical_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// The security policy for the web fetch tool, `web/fetch`.
///
/// The policy sets which URLs, ports, and addresses a fetch may reach and how
/// many redirects it may follow. It also sets the size caps on the response and
/// the returned text, the timeouts, and the `User-Agent` header.
///
/// A `FetchConfig` is always valid and immutable once built. Use
/// [`FetchConfig::default`] for the built-in safe policy, or start from
/// [`FetchConfig::builder`] to customize one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FetchConfig {
    /// Whether to permit `http://` URLs; `https://` is always allowed.
    allow_http: bool,
    /// The ports a fetch may target (matched against the URL's effective port).
    allow_ports: Vec<u16>,
    /// Whether to permit a host given as a bare IP literal.
    allow_ip_literals: bool,
    /// Extra CIDR ranges denied on top of the built-in blocked ranges.
    deny_extra: Vec<IpNet>,
    /// Exact host-plus-address exceptions allowed even when otherwise blocked.
    allow_exact: Vec<HostAddressException>,
    /// The maximum number of redirect hops a single fetch may follow.
    max_redirects: MaxRedirects,
    /// The largest response body accepted, counted on decompressed bytes.
    max_bytes: MaxBytes,
    /// The ceiling on returned text length, in characters.
    max_chars: MaxChars,
    /// The time allowed to establish a TCP connection on any hop.
    connect_timeout: PositiveDuration,
    /// The cap on the total time a single request may take.
    timeout: PositiveDuration,
    /// How long an idle pooled connection is kept before it is closed.
    pool_idle_timeout: PositiveDuration,
    /// The validated `User-Agent` header sent on every request.
    user_agent: UserAgent,
}

impl FetchConfig {
    /// Starts a builder seeded with the built-in default policy.
    #[must_use]
    pub(crate) fn builder() -> FetchConfigBuilder {
        FetchConfigBuilder::default()
    }

    /// Whether plain `http://` URLs are permitted.
    pub(crate) fn allow_http(&self) -> bool {
        self.allow_http
    }

    /// The ports a fetch may target.
    pub(crate) fn allow_ports(&self) -> &[u16] {
        &self.allow_ports
    }

    /// Whether a bare IP-literal host is permitted (syntax only; the address is
    /// still classified against the address policy).
    pub(crate) fn allow_ip_literals(&self) -> bool {
        self.allow_ip_literals
    }

    /// The extra denied CIDR ranges layered on the built-in table.
    pub(crate) fn deny_extra(&self) -> &[IpNet] {
        &self.deny_extra
    }

    /// The exact host-plus-address exceptions.
    pub(crate) fn allow_exact(&self) -> &[HostAddressException] {
        &self.allow_exact
    }

    /// The redirect-hop cap.
    pub(crate) fn max_redirects(&self) -> usize {
        self.max_redirects.0
    }

    /// The response-body byte cap.
    pub(crate) fn max_bytes(&self) -> usize {
        self.max_bytes.0
    }

    /// The returned-text character ceiling.
    pub(crate) fn max_chars(&self) -> usize {
        self.max_chars.0
    }

    /// The per-hop connect timeout.
    pub(crate) fn connect_timeout(&self) -> Duration {
        self.connect_timeout.0
    }

    /// The whole-request timeout.
    pub(crate) fn timeout(&self) -> Duration {
        self.timeout.0
    }

    /// The idle-connection pool timeout.
    pub(crate) fn pool_idle_timeout(&self) -> Duration {
        self.pool_idle_timeout.0
    }

    /// The validated `User-Agent` string.
    pub(crate) fn user_agent(&self) -> &str {
        &self.user_agent.0
    }
}

impl Default for FetchConfig {
    fn default() -> FetchConfig {
        // The constants below are all in range, so this construction is
        // infallible; the builder validates any caller-supplied override.
        FetchConfig {
            allow_http: false,
            allow_ports: DEFAULT_ALLOW_PORTS.to_vec(),
            allow_ip_literals: false,
            deny_extra: Vec::new(),
            allow_exact: Vec::new(),
            max_redirects: MaxRedirects(DEFAULT_MAX_REDIRECTS),
            max_bytes: MaxBytes(DEFAULT_MAX_BYTES),
            max_chars: MaxChars(DEFAULT_MAX_CHARS),
            connect_timeout: PositiveDuration(DEFAULT_CONNECT_TIMEOUT),
            timeout: PositiveDuration(DEFAULT_TIMEOUT),
            pool_idle_timeout: PositiveDuration(DEFAULT_POOL_IDLE_TIMEOUT),
            user_agent: UserAgent(DEFAULT_USER_AGENT.to_string()),
        }
    }
}

/// A builder for a custom [`FetchConfig`].
///
/// The builder starts from the built-in default policy. Each setter stores its
/// value as given and returns the builder, so calls can be chained.
/// [`FetchConfigBuilder::build`] checks every value at once and reports the
/// first invalid one as a [`ConfigError`].
#[derive(Debug, Clone)]
pub(crate) struct FetchConfigBuilder {
    allow_http: bool,
    allow_ports: Vec<u16>,
    allow_ip_literals: bool,
    deny_cidrs: Vec<String>,
    allow_hosts: Vec<(String, IpAddr)>,
    max_redirects: usize,
    max_bytes: usize,
    max_chars: usize,
    connect_timeout: Duration,
    timeout: Duration,
    pool_idle_timeout: Duration,
    user_agent: String,
}

impl Default for FetchConfigBuilder {
    fn default() -> FetchConfigBuilder {
        FetchConfigBuilder {
            allow_http: false,
            allow_ports: DEFAULT_ALLOW_PORTS.to_vec(),
            allow_ip_literals: false,
            deny_cidrs: Vec::new(),
            allow_hosts: Vec::new(),
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_bytes: DEFAULT_MAX_BYTES,
            max_chars: DEFAULT_MAX_CHARS,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            timeout: DEFAULT_TIMEOUT,
            pool_idle_timeout: DEFAULT_POOL_IDLE_TIMEOUT,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        }
    }
}

impl FetchConfigBuilder {
    /// Sets whether plain `http://` URLs are permitted.
    #[must_use]
    pub(crate) fn allow_http(mut self, yes: bool) -> FetchConfigBuilder {
        self.allow_http = yes;
        self
    }

    /// Replaces the set of ports a fetch may target.
    #[must_use]
    pub(crate) fn allow_ports(
        mut self,
        ports: impl IntoIterator<Item = u16>,
    ) -> FetchConfigBuilder {
        self.allow_ports = ports.into_iter().collect();
        self
    }

    /// Sets whether a URL may give its host as a bare IP address.
    ///
    /// This lifts only the syntax rule. The address itself is still checked
    /// against the blocked ranges. A loopback, private, link-local, or other
    /// non-global address stays blocked unless an exact host-and-address
    /// exception allows it.
    #[must_use]
    pub(crate) fn allow_ip_literals(mut self, yes: bool) -> FetchConfigBuilder {
        self.allow_ip_literals = yes;
        self
    }

    /// Adds a CIDR range to block, on top of the built-in blocked ranges.
    ///
    /// [`build`] parses the text and reports a range that fails to parse.
    ///
    /// [`build`]: FetchConfigBuilder::build
    #[must_use]
    pub(crate) fn deny_cidr(mut self, cidr: impl Into<String>) -> FetchConfigBuilder {
        self.deny_cidrs.push(cidr.into());
        self
    }

    /// Adds an exception that lets one host reach one otherwise-blocked address.
    ///
    /// The exception applies only when a fetch to `host` connects to `addr`.
    /// Another host that resolves to `addr` stays blocked. The host match
    /// ignores case and a trailing dot. [`build`] checks that `host` is a valid
    /// domain name or IP address. This is the only supported way to reach an
    /// otherwise-blocked address.
    ///
    /// [`build`]: FetchConfigBuilder::build
    #[must_use]
    pub(crate) fn allow_host_address(
        mut self,
        host: impl Into<String>,
        addr: IpAddr,
    ) -> FetchConfigBuilder {
        self.allow_hosts.push((host.into(), addr));
        self
    }

    /// Sets the maximum number of redirect hops a single fetch may follow.
    #[must_use]
    pub(crate) fn max_redirects(mut self, n: usize) -> FetchConfigBuilder {
        self.max_redirects = n;
        self
    }

    /// Sets the largest response body accepted, in decompressed bytes.
    #[must_use]
    pub(crate) fn max_bytes(mut self, n: usize) -> FetchConfigBuilder {
        self.max_bytes = n;
        self
    }

    /// Sets the ceiling on returned text length, in characters.
    #[must_use]
    pub(crate) fn max_chars(mut self, n: usize) -> FetchConfigBuilder {
        self.max_chars = n;
        self
    }

    /// Sets the time allowed to open a TCP connection, on every redirect hop.
    #[must_use]
    pub(crate) fn connect_timeout(mut self, d: Duration) -> FetchConfigBuilder {
        self.connect_timeout = d;
        self
    }

    /// Sets the cap on the total time a single request may take.
    #[must_use]
    pub(crate) fn timeout(mut self, d: Duration) -> FetchConfigBuilder {
        self.timeout = d;
        self
    }

    /// Sets how long an idle pooled connection is kept before it is closed.
    #[must_use]
    pub(crate) fn pool_idle_timeout(mut self, d: Duration) -> FetchConfigBuilder {
        self.pool_idle_timeout = d;
        self
    }

    /// Sets the `User-Agent` header sent on every request.
    #[must_use]
    pub(crate) fn user_agent(mut self, ua: impl Into<String>) -> FetchConfigBuilder {
        self.user_agent = ua.into();
        self
    }

    /// Checks every value and returns the finished [`FetchConfig`].
    ///
    /// # Errors
    /// Returns [`ConfigError`] for the first invalid value it finds:
    ///
    /// - a `User-Agent` that is not a legal HTTP header value;
    /// - a `max_bytes` of zero or above 64 MiB;
    /// - a `max_chars` of zero or above 10,000,000;
    /// - a `max_redirects` above 20;
    /// - a `connect_timeout` of zero or above 60 seconds;
    /// - a `timeout` of zero or above 300 seconds;
    /// - a `pool_idle_timeout` of zero or above 600 seconds;
    /// - a denied CIDR range that fails to parse;
    /// - an exception host that is not a valid domain name or IP address.
    pub(crate) fn build(self) -> Result<FetchConfig, ConfigError> {
        let user_agent = validate_user_agent(self.user_agent)?;
        let max_bytes = validate_limit("max_bytes", self.max_bytes, MAX_BYTES_CEILING)?;
        let max_chars = validate_limit("max_chars", self.max_chars, MAX_CHARS_CEILING)?;
        let max_redirects = validate_redirects(self.max_redirects)?;
        let connect_timeout =
            validate_timeout("connect_timeout", self.connect_timeout, MAX_CONNECT_TIMEOUT)?;
        let timeout = validate_timeout("timeout", self.timeout, MAX_TIMEOUT)?;
        let pool_idle_timeout = validate_timeout(
            "pool_idle_timeout",
            self.pool_idle_timeout,
            MAX_POOL_IDLE_TIMEOUT,
        )?;
        let deny_extra = validate_deny_cidrs(self.deny_cidrs)?;
        let allow_exact = validate_allow_hosts(self.allow_hosts)?;

        Ok(FetchConfig {
            allow_http: self.allow_http,
            allow_ports: self.allow_ports,
            allow_ip_literals: self.allow_ip_literals,
            deny_extra,
            allow_exact,
            max_redirects,
            max_bytes: MaxBytes(max_bytes),
            max_chars: MaxChars(max_chars),
            connect_timeout: PositiveDuration(connect_timeout),
            timeout: PositiveDuration(timeout),
            pool_idle_timeout: PositiveDuration(pool_idle_timeout),
            user_agent,
        })
    }
}

/// The error reported when a fetch configuration fails validation or its HTTP
/// client fails to build.
///
/// When a field fails validation, the [`Display`] message names the field and
/// the constraint it violated.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub(crate) struct ConfigError(#[from] ConfigErrorRepr);

/// The private representation behind [`ConfigError`].
#[derive(Debug, thiserror::Error)]
enum ConfigErrorRepr {
    /// The user agent is not a legal HTTP header value.
    #[error("user agent is not a valid http header value")]
    UserAgent(#[source] reqwest::header::InvalidHeaderValue),

    /// A limit was zero, which would disable the bound it governs.
    #[error("{field} must be greater than zero")]
    ZeroLimit {
        /// The name of the offending limit.
        field: &'static str,
    },

    /// A limit exceeded its hard operational ceiling.
    #[error("{field} ({value}) exceeds the maximum of {ceiling}")]
    OverCeiling {
        /// The name of the offending limit.
        field: &'static str,
        /// The rejected value.
        value: usize,
        /// The ceiling it exceeded.
        ceiling: usize,
    },

    /// A timeout was zero, which the policy does not allow.
    #[error("{field} must be a positive duration")]
    ZeroTimeout {
        /// The name of the offending timeout.
        field: &'static str,
    },

    /// A timeout exceeded its hard operational ceiling.
    #[error("{field} ({value:?}) exceeds the maximum of {ceiling:?}")]
    TimeoutOverCeiling {
        /// The name of the offending timeout.
        field: &'static str,
        /// The rejected duration.
        value: Duration,
        /// The ceiling it exceeded.
        ceiling: Duration,
    },

    /// A denied-CIDR string did not parse.
    #[error("invalid deny cidr {cidr}")]
    Cidr {
        /// The rejected CIDR text.
        cidr: String,
        /// The parse failure.
        #[source]
        source: ipnet::AddrParseError,
    },

    /// An exact-host exception named an empty or malformed host.
    #[error("invalid exact host {host:?}")]
    Host {
        /// The rejected host text.
        host: String,
    },

    /// The HTTP client could not be built for the validated policy.
    #[error("http client construction failed")]
    ClientBuild(#[source] reqwest::Error),
}

impl ConfigError {
    /// Builds a `ConfigError` from a reqwest client-build failure.
    pub(crate) fn client_build(source: reqwest::Error) -> ConfigError {
        ConfigError(ConfigErrorRepr::ClientBuild(source))
    }
}

/// Validates a `User-Agent` string as a legal HTTP header value.
fn validate_user_agent(ua: String) -> Result<UserAgent, ConfigErrorRepr> {
    HeaderValue::from_str(&ua).map_err(ConfigErrorRepr::UserAgent)?;
    Ok(UserAgent(ua))
}

/// Validates a positive limit against a hard ceiling.
fn validate_limit(
    field: &'static str,
    value: usize,
    ceiling: usize,
) -> Result<usize, ConfigErrorRepr> {
    if value == 0 {
        return Err(ConfigErrorRepr::ZeroLimit { field });
    }
    if value > ceiling {
        return Err(ConfigErrorRepr::OverCeiling {
            field,
            value,
            ceiling,
        });
    }
    Ok(value)
}

/// Validates the redirect cap against its ceiling; zero is permitted.
fn validate_redirects(value: usize) -> Result<MaxRedirects, ConfigErrorRepr> {
    if value > MAX_REDIRECTS_CEILING {
        return Err(ConfigErrorRepr::OverCeiling {
            field: "max_redirects",
            value,
            ceiling: MAX_REDIRECTS_CEILING,
        });
    }
    Ok(MaxRedirects(value))
}

/// Validates a timeout as strictly positive and within its ceiling.
fn validate_timeout(
    field: &'static str,
    value: Duration,
    ceiling: Duration,
) -> Result<Duration, ConfigErrorRepr> {
    if value.is_zero() {
        return Err(ConfigErrorRepr::ZeroTimeout { field });
    }
    if value > ceiling {
        return Err(ConfigErrorRepr::TimeoutOverCeiling {
            field,
            value,
            ceiling,
        });
    }
    Ok(value)
}

/// Parses and validates the denied-CIDR strings into networks.
fn validate_deny_cidrs(cidrs: Vec<String>) -> Result<Vec<IpNet>, ConfigErrorRepr> {
    let mut nets = Vec::with_capacity(cidrs.len());
    for cidr in cidrs {
        let net = cidr
            .parse::<IpNet>()
            .map_err(|source| ConfigErrorRepr::Cidr {
                cidr: cidr.clone(),
                source,
            })?;
        if !nets.contains(&net) {
            nets.push(net);
        }
    }
    Ok(nets)
}

/// Canonicalizes and validates one exact-exception host.
///
/// Accepts an IP literal (for a literal-host exception) or a syntactically valid
/// DNS domain. Every other form - empty, whitespace-, slash-, colon-, at-, or
/// query-bearing - is rejected by the URL host parser, which enforces the URL
/// forbidden-host-code-point set. Returns the canonical host to store.
fn validate_host(raw: &str) -> Result<String, ConfigErrorRepr> {
    let host = canonical_host(raw);
    if host.is_empty() {
        return Err(ConfigErrorRepr::Host {
            host: raw.to_string(),
        });
    }
    // An IP literal is a legitimate exact-exception host (a literal-host URL).
    if host.parse::<IpAddr>().is_ok() {
        return Ok(host);
    }
    // Otherwise require a valid DNS domain. `url::Host::parse` rejects every
    // forbidden host code point (`:` outside brackets, `@`, `?`, `#`, `/`,
    // whitespace, ...), so `bad:host`, `bad@host`, and `bad?host` are refused,
    // while a non-domain address form is not a valid exact host here.
    match url::Host::parse(&host) {
        Ok(url::Host::Domain(domain)) => Ok(domain),
        _ => Err(ConfigErrorRepr::Host {
            host: raw.to_string(),
        }),
    }
}

/// Canonicalizes and validates the exact host-plus-address exceptions.
fn validate_allow_hosts(
    hosts: Vec<(String, IpAddr)>,
) -> Result<Vec<HostAddressException>, ConfigErrorRepr> {
    let mut out: Vec<HostAddressException> = Vec::with_capacity(hosts.len());
    for (raw, addr) in hosts {
        let host = validate_host(&raw)?;
        let entry = HostAddressException { host, addr };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "config-tests.rs"]
mod tests;
