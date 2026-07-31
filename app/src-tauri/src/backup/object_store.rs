#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::{
    fmt,
    io::{Cursor, Read},
    time::Duration,
};

use reqwest::{
    blocking::{Body, Client, Response},
    header::{HeaderMap, HeaderValue, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH},
    redirect::Policy,
    StatusCode,
};
use rusty_s3::{actions::ListObjectsV2, Bucket, Credentials, S3Action, UrlStyle};
use sha2::{Digest, Sha256};

use super::{
    credentials::R2Credentials,
    domain::{
        ContentSha256, ProbeRunId, R2Keyspace, R2ListPrefix, R2ObjectKey, R2Target,
        OBJECT_FORMAT_VERSION,
    },
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SIGNED_URL_LIFETIME: Duration = Duration::from_secs(60);
pub(crate) const MAX_OBJECT_BYTES: usize = 256 * 1024 * 1024;
const MAX_LIST_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_LIST_OBJECTS: usize = 1_000;
const MAX_ETAG_BYTES: usize = 256;
const MAX_CONTINUATION_TOKEN_BYTES: usize = 4 * 1024;
const KOSH_SHA256_HEADER: &str = "x-amz-meta-kosh-sha256";
const KOSH_OBJECT_FORMAT_HEADER: &str = "x-amz-meta-kosh-object-format";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObjectContentType {
    Binary,
    Json,
}

impl ObjectContentType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "application/octet-stream",
            Self::Json => "application/json",
        }
    }

    fn parse(value: Option<&HeaderValue>) -> Result<Option<Self>, ObjectStoreError> {
        let Some(value) = value else {
            return Ok(None);
        };
        match value.to_str().map_err(|_| invalid_response())? {
            "application/octet-stream" => Ok(Some(Self::Binary)),
            "application/json" => Ok(Some(Self::Json)),
            _ => Err(invalid_response()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObjectVersion(String);

impl ObjectVersion {
    fn parse(value: impl Into<String>) -> Result<Self, ObjectStoreError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_ETAG_BYTES || value.chars().any(char::is_control) {
            return Err(invalid_response());
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObjectMetadata {
    pub(crate) byte_length: u64,
    pub(crate) version: ObjectVersion,
    pub(crate) content_type: Option<ObjectContentType>,
    pub(crate) kosh_sha256: Option<ContentSha256>,
    pub(crate) object_format_version: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GetObjectResult {
    pub(crate) metadata: ObjectMetadata,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PutCondition {
    IfAbsent,
    IfMatch(ObjectVersion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PutObjectOutcome {
    Stored,
    ConditionNotMet,
}

#[derive(Debug)]
pub(crate) struct PutObjectRequest {
    pub(crate) key: R2ObjectKey,
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_type: ObjectContentType,
    pub(crate) kosh_sha256: Option<ContentSha256>,
    pub(crate) condition: PutCondition,
}

#[derive(Debug)]
pub(crate) struct PutMediaRequest {
    key: R2ObjectKey,
    bytes: Vec<u8>,
    expected_sha256: ContentSha256,
}

impl PutMediaRequest {
    pub(crate) fn new(
        keyspace: &R2Keyspace,
        expected_sha256: ContentSha256,
        bytes: Vec<u8>,
    ) -> Result<Self, ObjectStoreError> {
        if bytes.len() > MAX_OBJECT_BYTES {
            return Err(ObjectStoreError::new(ObjectStoreErrorCode::ObjectTooLarge));
        }
        if ContentSha256::from_bytes(Sha256::digest(&bytes).into()) != expected_sha256 {
            return Err(ObjectStoreError::new(
                ObjectStoreErrorCode::ContentHashMismatch,
            ));
        }
        Ok(Self {
            key: keyspace.media(expected_sha256),
            bytes,
            expected_sha256,
        })
    }

    fn into_object_request(
        self,
        keyspace: &R2Keyspace,
    ) -> Result<PutObjectRequest, ObjectStoreError> {
        if self.key != keyspace.media(self.expected_sha256)
            || ContentSha256::from_bytes(Sha256::digest(&self.bytes).into()) != self.expected_sha256
        {
            return Err(ObjectStoreError::new(
                ObjectStoreErrorCode::ContentHashMismatch,
            ));
        }
        Ok(PutObjectRequest {
            key: self.key,
            bytes: self.bytes,
            content_type: ObjectContentType::Binary,
            kosh_sha256: Some(self.expected_sha256),
            condition: PutCondition::IfAbsent,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ListedObject {
    pub(crate) key: R2ObjectKey,
    pub(crate) byte_length: u64,
    pub(crate) version: ObjectVersion,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContinuationToken(String);

impl ContinuationToken {
    fn parse(value: impl Into<String>) -> Result<Self, ObjectStoreError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CONTINUATION_TOKEN_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(invalid_response());
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ListObjectsPage {
    pub(crate) objects: Vec<ListedObject>,
    pub(crate) next: Option<ContinuationToken>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProbeDeleteAuthorization {
    prefix: R2ListPrefix,
}

impl ProbeDeleteAuthorization {
    pub(crate) fn for_run(keyspace: &R2Keyspace, run_id: &ProbeRunId) -> Self {
        Self {
            prefix: keyspace.probe_prefix(run_id),
        }
    }

    fn authorize(&self, key: &R2ObjectKey) -> Result<(), ObjectStoreError> {
        if key.as_str().starts_with(self.prefix.as_str()) {
            Ok(())
        } else {
            Err(ObjectStoreError::new(
                ObjectStoreErrorCode::DeletionNotAuthorized,
            ))
        }
    }
}

pub(crate) trait ObjectStore: Send + Sync {
    fn head(&self, key: &R2ObjectKey) -> Result<Option<ObjectMetadata>, ObjectStoreError>;
    fn get(&self, key: &R2ObjectKey) -> Result<GetObjectResult, ObjectStoreError>;
    fn get_bounded(
        &self,
        key: &R2ObjectKey,
        max_bytes: usize,
    ) -> Result<GetObjectResult, ObjectStoreError> {
        let result = self.get(key)?;
        if result.bytes.len() > max_bytes {
            return Err(ObjectStoreError::new(
                ObjectStoreErrorCode::ResponseTooLarge,
            ));
        }
        Ok(result)
    }
    fn put(&self, request: PutObjectRequest) -> Result<PutObjectOutcome, ObjectStoreError>;
    fn put_media(&self, request: PutMediaRequest) -> Result<PutObjectOutcome, ObjectStoreError>;
    fn list(
        &self,
        prefix: &R2ListPrefix,
        continuation: Option<&ContinuationToken>,
    ) -> Result<ListObjectsPage, ObjectStoreError>;
    fn delete_probe(
        &self,
        authorization: &ProbeDeleteAuthorization,
        key: &R2ObjectKey,
    ) -> Result<(), ObjectStoreError>;
}

pub(crate) struct R2ObjectStore {
    client: Client,
    bucket: Bucket,
    credentials: Credentials,
    keyspace: R2Keyspace,
}

impl fmt::Debug for R2ObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("R2ObjectStore")
            .field("bucket", &self.bucket.name())
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl R2ObjectStore {
    pub(crate) fn new(
        target: R2Target,
        keyspace: R2Keyspace,
        credentials: &R2Credentials,
    ) -> Result<Self, ObjectStoreError> {
        let endpoint = target
            .endpoint()
            .parse()
            .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::InvalidConfiguration))?;
        let bucket = Bucket::new(
            endpoint,
            UrlStyle::VirtualHost,
            target.bucket.as_str().to_owned(),
            "auto",
        )
        .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::InvalidConfiguration))?;
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .https_only(true)
            .build()
            .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::InvalidConfiguration))?;
        Ok(Self {
            client,
            bucket,
            credentials: Credentials::new(
                credentials.access_key_id().to_owned(),
                credentials.secret_access_key().to_owned(),
            ),
            keyspace,
        })
    }

    fn validate_key(&self, key: &R2ObjectKey) -> Result<(), ObjectStoreError> {
        self.keyspace
            .validate_returned_key(key.as_str())
            .map(|_| ())
            .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::KeyOutsidePrefix))
    }

    fn validate_prefix(&self, prefix: &R2ListPrefix) -> Result<(), ObjectStoreError> {
        self.keyspace
            .validate_list_prefix(prefix)
            .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::KeyOutsidePrefix))
    }

    fn send(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<Response, ObjectStoreError> {
        request.send().map_err(|error| {
            if error.is_timeout() {
                ObjectStoreError::new(ObjectStoreErrorCode::Timeout)
            } else {
                ObjectStoreError::new(ObjectStoreErrorCode::Network)
            }
        })
    }

    fn put_validated(
        &self,
        request: PutObjectRequest,
    ) -> Result<PutObjectOutcome, ObjectStoreError> {
        self.validate_key(&request.key)?;
        if request.bytes.len() > MAX_OBJECT_BYTES {
            return Err(ObjectStoreError::new(ObjectStoreErrorCode::ObjectTooLarge));
        }
        let byte_length = request.bytes.len();
        let credentials = self.credentials.clone();
        let mut action = self
            .bucket
            .put_object(Some(&credentials), request.key.as_str());
        action
            .headers_mut()
            .insert("content-type", request.content_type.as_str());
        action
            .headers_mut()
            .insert("content-length", byte_length.to_string());
        action
            .headers_mut()
            .insert(KOSH_OBJECT_FORMAT_HEADER, OBJECT_FORMAT_VERSION.to_string());
        let sha256 = request.kosh_sha256.map(ContentSha256::to_hex);
        if let Some(sha256) = &sha256 {
            action
                .headers_mut()
                .insert(KOSH_SHA256_HEADER, sha256.clone());
        }
        match &request.condition {
            PutCondition::IfAbsent => {
                action.headers_mut().insert("if-none-match", "*");
            }
            PutCondition::IfMatch(version) => {
                action
                    .headers_mut()
                    .insert("if-match", version.as_str().to_owned());
            }
        }
        let url = action.sign(SIGNED_URL_LIFETIME);
        let mut builder = self
            .client
            .put(url)
            .header(CONTENT_TYPE, request.content_type.as_str())
            .header(CONTENT_LENGTH, byte_length)
            .header(KOSH_OBJECT_FORMAT_HEADER, OBJECT_FORMAT_VERSION)
            .body(Body::new(Cursor::new(request.bytes)));
        if let Some(sha256) = sha256 {
            builder = builder.header(KOSH_SHA256_HEADER, sha256);
        }
        builder = match request.condition {
            PutCondition::IfAbsent => builder.header(IF_NONE_MATCH, "*"),
            PutCondition::IfMatch(version) => builder.header(IF_MATCH, version.as_str()),
        };
        let response = self.send(builder)?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Ok(PutObjectOutcome::ConditionNotMet);
        }
        ensure_success(response.status())?;
        parse_version(response.headers())?;
        Ok(PutObjectOutcome::Stored)
    }

    fn get_with_limit(
        &self,
        key: &R2ObjectKey,
        max_bytes: usize,
    ) -> Result<GetObjectResult, ObjectStoreError> {
        self.validate_key(key)?;
        if max_bytes > MAX_OBJECT_BYTES {
            return Err(ObjectStoreError::new(
                ObjectStoreErrorCode::ResponseTooLarge,
            ));
        }
        let credentials = self.credentials.clone();
        let action = self.bucket.get_object(Some(&credentials), key.as_str());
        let response = self.send(self.client.get(action.sign(SIGNED_URL_LIFETIME)))?;
        ensure_success(response.status())?;
        let metadata = parse_metadata(response.headers())?;
        let bytes = read_bounded(response, max_bytes)?;
        if bytes.len() as u64 != metadata.byte_length {
            return Err(invalid_response());
        }
        Ok(GetObjectResult { metadata, bytes })
    }

    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn delete_canary_object(&self, key: &R2ObjectKey) -> Result<(), ObjectStoreError> {
        if std::env::var("KOSH_RUN_R2_CANARY").as_deref() != Ok("1") {
            return Err(ObjectStoreError::new(
                ObjectStoreErrorCode::DeletionNotAuthorized,
            ));
        }
        self.validate_key(key)?;
        let credentials = self.credentials.clone();
        let action = self.bucket.delete_object(Some(&credentials), key.as_str());
        let response = self.send(self.client.delete(action.sign(SIGNED_URL_LIFETIME)))?;
        ensure_success(response.status())
    }
}

impl ObjectStore for R2ObjectStore {
    fn head(&self, key: &R2ObjectKey) -> Result<Option<ObjectMetadata>, ObjectStoreError> {
        self.validate_key(key)?;
        let credentials = self.credentials.clone();
        let action = self.bucket.head_object(Some(&credentials), key.as_str());
        let response = self.send(self.client.head(action.sign(SIGNED_URL_LIFETIME)))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure_success(response.status())?;
        Ok(Some(parse_metadata(response.headers())?))
    }

    fn get(&self, key: &R2ObjectKey) -> Result<GetObjectResult, ObjectStoreError> {
        self.get_with_limit(key, MAX_OBJECT_BYTES)
    }

    fn get_bounded(
        &self,
        key: &R2ObjectKey,
        max_bytes: usize,
    ) -> Result<GetObjectResult, ObjectStoreError> {
        self.get_with_limit(key, max_bytes)
    }

    fn put(&self, request: PutObjectRequest) -> Result<PutObjectOutcome, ObjectStoreError> {
        if self.keyspace.is_media_key(&request.key) {
            return Err(ObjectStoreError::new(
                ObjectStoreErrorCode::MediaWriteRequiresVerifiedCreate,
            ));
        }
        self.put_validated(request)
    }

    fn put_media(&self, request: PutMediaRequest) -> Result<PutObjectOutcome, ObjectStoreError> {
        self.put_validated(request.into_object_request(&self.keyspace)?)
    }

    fn list(
        &self,
        prefix: &R2ListPrefix,
        continuation: Option<&ContinuationToken>,
    ) -> Result<ListObjectsPage, ObjectStoreError> {
        self.validate_prefix(prefix)?;
        let credentials = self.credentials.clone();
        let mut action = self.bucket.list_objects_v2(Some(&credentials));
        action.with_prefix(prefix.as_str().to_owned());
        action.with_max_keys(MAX_LIST_OBJECTS);
        if let Some(continuation) = continuation {
            action.with_continuation_token(continuation.as_str().to_owned());
        }
        let response = self.send(self.client.get(action.sign(SIGNED_URL_LIFETIME)))?;
        ensure_success(response.status())?;
        let bytes = read_bounded(response, MAX_LIST_RESPONSE_BYTES)?;
        let body = std::str::from_utf8(&bytes).map_err(|_| invalid_response())?;
        let parsed = ListObjectsV2::parse_response(body).map_err(|_| invalid_response())?;
        if parsed.contents.len() > MAX_LIST_OBJECTS {
            return Err(invalid_response());
        }
        let objects = parsed
            .contents
            .into_iter()
            .map(|object| {
                if !object.key.starts_with(prefix.as_str()) {
                    return Err(ObjectStoreError::new(
                        ObjectStoreErrorCode::KeyOutsidePrefix,
                    ));
                }
                Ok(ListedObject {
                    key: self
                        .keyspace
                        .validate_returned_key(object.key)
                        .map_err(|_| {
                            ObjectStoreError::new(ObjectStoreErrorCode::KeyOutsidePrefix)
                        })?,
                    byte_length: object.size,
                    version: ObjectVersion::parse(object.etag)?,
                })
            })
            .collect::<Result<Vec<_>, ObjectStoreError>>()?;
        let next = parsed
            .next_continuation_token
            .map(ContinuationToken::parse)
            .transpose()?;
        Ok(ListObjectsPage { objects, next })
    }

    fn delete_probe(
        &self,
        authorization: &ProbeDeleteAuthorization,
        key: &R2ObjectKey,
    ) -> Result<(), ObjectStoreError> {
        self.validate_key(key)?;
        authorization.authorize(key)?;
        let credentials = self.credentials.clone();
        let action = self.bucket.delete_object(Some(&credentials), key.as_str());
        let response = self.send(self.client.delete(action.sign(SIGNED_URL_LIFETIME)))?;
        ensure_success(response.status())
    }
}

fn parse_metadata(headers: &HeaderMap) -> Result<ObjectMetadata, ObjectStoreError> {
    let byte_length = headers
        .get(CONTENT_LENGTH)
        .ok_or_else(invalid_response)?
        .to_str()
        .map_err(|_| invalid_response())?
        .parse::<u64>()
        .map_err(|_| invalid_response())?;
    let version = parse_version(headers)?;
    let content_type = ObjectContentType::parse(headers.get(CONTENT_TYPE))?;
    let kosh_sha256 = headers
        .get(KOSH_SHA256_HEADER)
        .map(|value| {
            let value = value.to_str().map_err(|_| invalid_response())?;
            ContentSha256::parse_hex(value).map_err(|_| invalid_response())
        })
        .transpose()?;
    let object_format_version = headers
        .get(KOSH_OBJECT_FORMAT_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| invalid_response())?
                .parse::<u32>()
                .map_err(|_| invalid_response())
        })
        .transpose()?;
    Ok(ObjectMetadata {
        byte_length,
        version,
        content_type,
        kosh_sha256,
        object_format_version,
    })
}

fn parse_version(headers: &HeaderMap) -> Result<ObjectVersion, ObjectStoreError> {
    let etag = headers
        .get(ETAG)
        .ok_or_else(invalid_response)?
        .to_str()
        .map_err(|_| invalid_response())?;
    ObjectVersion::parse(etag)
}

fn read_bounded(mut response: Response, limit: usize) -> Result<Vec<u8>, ObjectStoreError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ObjectStoreError::new(
            ObjectStoreErrorCode::ResponseTooLarge,
        ));
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::Network))?;
    if bytes.len() > limit {
        return Err(ObjectStoreError::new(
            ObjectStoreErrorCode::ResponseTooLarge,
        ));
    }
    Ok(bytes)
}

fn ensure_success(status: StatusCode) -> Result<(), ObjectStoreError> {
    if status.is_success() {
        return Ok(());
    }
    let code = match status {
        StatusCode::UNAUTHORIZED => ObjectStoreErrorCode::AuthenticationRejected,
        StatusCode::FORBIDDEN => ObjectStoreErrorCode::AuthorizationRejected,
        StatusCode::NOT_FOUND => ObjectStoreErrorCode::NotFound,
        StatusCode::CONFLICT => ObjectStoreErrorCode::Conflict,
        StatusCode::PRECONDITION_FAILED => ObjectStoreErrorCode::PreconditionFailed,
        StatusCode::TOO_MANY_REQUESTS => ObjectStoreErrorCode::RateLimited,
        status if status.is_server_error() => ObjectStoreErrorCode::ServiceUnavailable,
        _ => ObjectStoreErrorCode::InvalidResponse,
    };
    Err(ObjectStoreError::new(code))
}

fn invalid_response() -> ObjectStoreError {
    ObjectStoreError::new(ObjectStoreErrorCode::InvalidResponse)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObjectStoreErrorCode {
    InvalidConfiguration,
    KeyOutsidePrefix,
    DeletionNotAuthorized,
    MediaWriteRequiresVerifiedCreate,
    ContentHashMismatch,
    Network,
    Timeout,
    AuthenticationRejected,
    AuthorizationRejected,
    NotFound,
    Conflict,
    PreconditionFailed,
    RateLimited,
    ServiceUnavailable,
    ObjectTooLarge,
    ResponseTooLarge,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("R2 object-store operation failed: {code:?}")]
pub(crate) struct ObjectStoreError {
    pub(crate) code: ObjectStoreErrorCode,
}

impl ObjectStoreError {
    pub(crate) const fn new(code: ObjectStoreErrorCode) -> Self {
        Self { code }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::{
        collections::{BTreeMap, VecDeque},
        sync::Mutex,
    };

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum ObjectOperation {
        Head,
        Get,
        Put,
        List,
        Delete,
    }

    #[derive(Clone)]
    struct StoredObject {
        key: R2ObjectKey,
        bytes: Vec<u8>,
        metadata: ObjectMetadata,
    }

    #[derive(Default)]
    struct FakeState {
        objects: BTreeMap<String, StoredObject>,
        failures: VecDeque<(ObjectOperation, ObjectStoreErrorCode)>,
        operations: Vec<ObjectOperation>,
    }

    pub(crate) struct FakeObjectStore {
        keyspace: R2Keyspace,
        state: Mutex<FakeState>,
    }

    impl FakeObjectStore {
        pub(crate) fn new(keyspace: R2Keyspace) -> Self {
            Self {
                keyspace,
                state: Mutex::new(FakeState::default()),
            }
        }

        pub(crate) fn fail_next(&self, operation: ObjectOperation, code: ObjectStoreErrorCode) {
            self.state
                .lock()
                .expect("fake object store")
                .failures
                .push_back((operation, code));
        }

        pub(crate) fn operations(&self) -> Vec<ObjectOperation> {
            self.state
                .lock()
                .expect("fake object store")
                .operations
                .clone()
        }

        pub(crate) fn clear_operations(&self) {
            self.state
                .lock()
                .expect("fake object store")
                .operations
                .clear();
        }

        pub(crate) fn remove_for_test(&self, key: &R2ObjectKey) {
            self.validate_key(key).expect("valid fake object key");
            self.state
                .lock()
                .expect("fake object store")
                .objects
                .remove(key.as_str());
        }

        fn validate_key(&self, key: &R2ObjectKey) -> Result<(), ObjectStoreError> {
            self.keyspace
                .validate_returned_key(key.as_str())
                .map(|_| ())
                .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::KeyOutsidePrefix))
        }

        fn begin(
            state: &mut FakeState,
            operation: ObjectOperation,
        ) -> Result<(), ObjectStoreError> {
            state.operations.push(operation);
            if state
                .failures
                .front()
                .is_some_and(|(expected, _)| *expected == operation)
            {
                let (_, code) = state.failures.pop_front().expect("queued failure");
                return Err(ObjectStoreError::new(code));
            }
            Ok(())
        }

        fn put_validated(
            &self,
            request: PutObjectRequest,
        ) -> Result<PutObjectOutcome, ObjectStoreError> {
            self.validate_key(&request.key)?;
            let mut state = self.state.lock().expect("fake object store");
            Self::begin(&mut state, ObjectOperation::Put)?;
            let current = state.objects.get(request.key.as_str());
            let condition_met = match &request.condition {
                PutCondition::IfAbsent => current.is_none(),
                PutCondition::IfMatch(expected) => {
                    current.is_some_and(|object| object.metadata.version == *expected)
                }
            };
            if !condition_met {
                return Ok(PutObjectOutcome::ConditionNotMet);
            }
            let version = ObjectVersion::parse(format!(
                "\"{}\"",
                ContentSha256::from_bytes(Sha256::digest(&request.bytes).into()).to_hex()
            ))?;
            let byte_length = request.bytes.len() as u64;
            state.objects.insert(
                request.key.as_str().to_owned(),
                StoredObject {
                    key: request.key.clone(),
                    bytes: request.bytes,
                    metadata: ObjectMetadata {
                        byte_length,
                        version,
                        content_type: Some(request.content_type),
                        kosh_sha256: request.kosh_sha256,
                        object_format_version: Some(OBJECT_FORMAT_VERSION),
                    },
                },
            );
            Ok(PutObjectOutcome::Stored)
        }
    }

    impl ObjectStore for FakeObjectStore {
        fn head(&self, key: &R2ObjectKey) -> Result<Option<ObjectMetadata>, ObjectStoreError> {
            self.validate_key(key)?;
            let mut state = self.state.lock().expect("fake object store");
            Self::begin(&mut state, ObjectOperation::Head)?;
            Ok(state
                .objects
                .get(key.as_str())
                .map(|object| object.metadata.clone()))
        }

        fn get(&self, key: &R2ObjectKey) -> Result<GetObjectResult, ObjectStoreError> {
            self.validate_key(key)?;
            let mut state = self.state.lock().expect("fake object store");
            Self::begin(&mut state, ObjectOperation::Get)?;
            let object = state
                .objects
                .get(key.as_str())
                .ok_or_else(|| ObjectStoreError::new(ObjectStoreErrorCode::NotFound))?;
            Ok(GetObjectResult {
                metadata: object.metadata.clone(),
                bytes: object.bytes.clone(),
            })
        }

        fn put(&self, request: PutObjectRequest) -> Result<PutObjectOutcome, ObjectStoreError> {
            if self.keyspace.is_media_key(&request.key) {
                return Err(ObjectStoreError::new(
                    ObjectStoreErrorCode::MediaWriteRequiresVerifiedCreate,
                ));
            }
            self.put_validated(request)
        }

        fn put_media(
            &self,
            request: PutMediaRequest,
        ) -> Result<PutObjectOutcome, ObjectStoreError> {
            self.put_validated(request.into_object_request(&self.keyspace)?)
        }

        fn list(
            &self,
            prefix: &R2ListPrefix,
            _continuation: Option<&ContinuationToken>,
        ) -> Result<ListObjectsPage, ObjectStoreError> {
            self.keyspace
                .validate_list_prefix(prefix)
                .map_err(|_| ObjectStoreError::new(ObjectStoreErrorCode::KeyOutsidePrefix))?;
            let mut state = self.state.lock().expect("fake object store");
            Self::begin(&mut state, ObjectOperation::List)?;
            let objects = state
                .objects
                .iter()
                .filter(|(key, _)| key.starts_with(prefix.as_str()))
                .map(|(_, object)| ListedObject {
                    key: object.key.clone(),
                    byte_length: object.metadata.byte_length,
                    version: object.metadata.version.clone(),
                })
                .collect();
            Ok(ListObjectsPage {
                objects,
                next: None,
            })
        }

        fn delete_probe(
            &self,
            authorization: &ProbeDeleteAuthorization,
            key: &R2ObjectKey,
        ) -> Result<(), ObjectStoreError> {
            self.validate_key(key)?;
            authorization.authorize(key)?;
            let mut state = self.state.lock().expect("fake object store");
            Self::begin(&mut state, ObjectOperation::Delete)?;
            state.objects.remove(key.as_str());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fake::{FakeObjectStore, ObjectOperation},
        *,
    };
    use crate::backup::domain::{BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction};

    fn target() -> R2Target {
        R2Target {
            account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef").expect("account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-local").expect("bucket"),
        }
    }

    #[test]
    fn fake_enforces_conditions_and_all_object_operations() {
        let keyspace = target().keyspace(&BackupSetId::new());
        let store = FakeObjectStore::new(keyspace.clone());
        let run_id = ProbeRunId::new();
        let key = keyspace.probe_object(&run_id);
        let delete_authorization = ProbeDeleteAuthorization::for_run(&keyspace, &run_id);
        let request = || PutObjectRequest {
            key: key.clone(),
            bytes: b"payload".to_vec(),
            content_type: ObjectContentType::Binary,
            kosh_sha256: None,
            condition: PutCondition::IfAbsent,
        };
        assert_eq!(
            store.put(request()).expect("first put"),
            PutObjectOutcome::Stored
        );
        assert_eq!(
            store.put(request()).expect("second put"),
            PutObjectOutcome::ConditionNotMet
        );
        let head = store.head(&key).expect("head").expect("metadata");
        assert_eq!(head.byte_length, 7);
        assert_eq!(store.get(&key).expect("get").bytes, b"payload");
        assert_eq!(
            store
                .list(&keyspace.root_prefix(), None)
                .expect("list")
                .objects
                .len(),
            1
        );
        store
            .delete_probe(&delete_authorization, &key)
            .expect("delete");
        assert!(store.head(&key).expect("head after delete").is_none());
        assert_eq!(
            store.operations(),
            [
                ObjectOperation::Put,
                ObjectOperation::Put,
                ObjectOperation::Head,
                ObjectOperation::Get,
                ObjectOperation::List,
                ObjectOperation::Delete,
                ObjectOperation::Head,
            ]
        );
    }

    #[test]
    fn probe_deletion_authorization_cannot_delete_non_probe_objects() {
        let keyspace = target().keyspace(&BackupSetId::new());
        let store = FakeObjectStore::new(keyspace.clone());
        let run_id = ProbeRunId::new();
        let authorization = ProbeDeleteAuthorization::for_run(&keyspace, &run_id);
        let identity = keyspace.identity();
        let media = keyspace.media(ContentSha256::from_bytes([0xab; 32]));

        for (label, key) in [("identity", identity), ("media", media)] {
            let error = store.delete_probe(&authorization, &key).expect_err(label);
            assert_eq!(error.code, ObjectStoreErrorCode::DeletionNotAuthorized);
        }
        assert!(store.operations().is_empty());
    }

    #[test]
    fn media_uploads_are_hash_verified_create_only_and_not_generic_writes() {
        let keyspace = target().keyspace(&BackupSetId::new());
        let store = FakeObjectStore::new(keyspace.clone());
        let payload = b"immutable media bytes".to_vec();
        let digest = ContentSha256::from_bytes(Sha256::digest(&payload).into());
        assert_eq!(
            PutMediaRequest::new(
                &keyspace,
                ContentSha256::from_bytes([0xab; 32]),
                payload.clone(),
            )
            .expect_err("mismatched media digest")
            .code,
            ObjectStoreErrorCode::ContentHashMismatch
        );

        assert_eq!(
            store
                .put_media(
                    PutMediaRequest::new(&keyspace, digest, payload.clone())
                        .expect("verified media request"),
                )
                .expect("first media put"),
            PutObjectOutcome::Stored
        );
        let key = keyspace.media(digest);
        let version = store
            .head(&key)
            .expect("media head")
            .expect("stored media")
            .version;
        assert_eq!(
            store
                .put(PutObjectRequest {
                    key: key.clone(),
                    bytes: b"replacement".to_vec(),
                    content_type: ObjectContentType::Binary,
                    kosh_sha256: Some(digest),
                    condition: PutCondition::IfMatch(version),
                })
                .expect_err("generic media replacement")
                .code,
            ObjectStoreErrorCode::MediaWriteRequiresVerifiedCreate
        );
        assert_eq!(
            store
                .put_media(
                    PutMediaRequest::new(&keyspace, digest, payload.clone())
                        .expect("verified duplicate request"),
                )
                .expect("duplicate media put"),
            PutObjectOutcome::ConditionNotMet
        );
        assert_eq!(store.get(&key).expect("immutable media").bytes, payload);
    }

    #[test]
    fn fake_supports_etag_guarded_replacement() {
        let keyspace = target().keyspace(&BackupSetId::new());
        let store = FakeObjectStore::new(keyspace.clone());
        let key = keyspace.owner();
        let first = PutObjectRequest {
            key: key.clone(),
            bytes: b"owner-one".to_vec(),
            content_type: ObjectContentType::Json,
            kosh_sha256: None,
            condition: PutCondition::IfAbsent,
        };
        assert_eq!(
            store.put(first).expect("initial owner"),
            PutObjectOutcome::Stored
        );
        let original = store.head(&key).expect("head").expect("owner");
        assert_eq!(
            store
                .put(PutObjectRequest {
                    key: key.clone(),
                    bytes: b"owner-two".to_vec(),
                    content_type: ObjectContentType::Json,
                    kosh_sha256: None,
                    condition: PutCondition::IfMatch(original.version.clone()),
                })
                .expect("guarded replacement"),
            PutObjectOutcome::Stored
        );
        assert_eq!(
            store
                .put(PutObjectRequest {
                    key: key.clone(),
                    bytes: b"stale-owner".to_vec(),
                    content_type: ObjectContentType::Json,
                    kosh_sha256: None,
                    condition: PutCondition::IfMatch(original.version),
                })
                .expect("stale replacement"),
            PutObjectOutcome::ConditionNotMet
        );
        assert_eq!(store.get(&key).expect("current owner").bytes, b"owner-two");
    }

    #[test]
    fn fake_confines_keys_and_supports_deterministic_failures() {
        let first = target().keyspace(&BackupSetId::new());
        let second = target().keyspace(&BackupSetId::new());
        let store = FakeObjectStore::new(first.clone());
        assert_eq!(
            store
                .head(&second.identity())
                .expect_err("foreign key rejected")
                .code,
            ObjectStoreErrorCode::KeyOutsidePrefix
        );
        store.fail_next(ObjectOperation::Head, ObjectStoreErrorCode::RateLimited);
        assert_eq!(
            store
                .head(&first.identity())
                .expect_err("injected failure")
                .code,
            ObjectStoreErrorCode::RateLimited
        );
    }
}
