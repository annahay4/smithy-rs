/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use aws_config::Region;
use aws_runtime::{
    sdk_feature::AwsSdkFeature, user_agent::test_util::assert_ua_contains_metric_values,
};
use aws_sdk_s3::{
    config::{Intercept, IntoShared},
    primitives::ByteStream,
    Client, Config,
};
use aws_smithy_http_client::test_util::capture_request;
use serial_test::serial;

#[derive(Debug)]
struct TransferManagerFeatureInterceptor;

impl Intercept for TransferManagerFeatureInterceptor {
    fn name(&self) -> &'static str {
        "TransferManagerFeature"
    }

    fn read_before_execution(
        &self,
        _ctx: &aws_sdk_s3::config::interceptors::BeforeSerializationInterceptorContextRef<'_>,
        cfg: &mut aws_sdk_s3::config::ConfigBag,
    ) -> Result<(), aws_sdk_s3::error::BoxError> {
        cfg.interceptor_state()
            .store_append(AwsSdkFeature::S3Transfer);
        Ok(())
    }
}

#[tokio::test]
async fn test_track_metric_for_s3_transfer_manager() {
    let (http_client, captured_request) = capture_request(None);
    let mut conf_builder = Config::builder()
        .region(Region::new("us-east-1"))
        .http_client(http_client.clone())
        .with_test_defaults();
    // The S3 Transfer Manager uses a passed-in S3 client SDK for operations.
    // By configuring an interceptor at the client level to track metrics,
    // all operations executed by the client will automatically include the metric.
    // This eliminates the need to apply `.config_override` on individual operations
    // to insert the `TransferManagerFeatureInterceptor`.
    conf_builder.push_interceptor(TransferManagerFeatureInterceptor.into_shared());
    let client = Client::from_conf(conf_builder.build());

    let _ = client
        .put_object()
        .bucket("doesnotmatter")
        .key("doesnotmatter")
        .body(ByteStream::from_static("Hello, world".as_bytes()))
        .send()
        .await
        .unwrap();

    let expected_req = captured_request.expect_request();
    let user_agent = expected_req.headers().get("x-amz-user-agent").unwrap();
    assert_ua_contains_metric_values(user_agent, &["G"]);
}

#[tokio::test]
async fn test_endpoint_override_tracking() {
    let (http_client, captured_request) = capture_request(None);
    let config = Config::builder()
        .region(Region::new("us-east-1"))
        .http_client(http_client.clone())
        .endpoint_url("http://localhost:9000")
        .with_test_defaults()
        .build();
    let client = Client::from_conf(config);

    let _ = client
        .put_object()
        .bucket("test-bucket")
        .key("test-key")
        .body(ByteStream::from_static("test data".as_bytes()))
        .send()
        .await;

    let expected_req = captured_request.expect_request();
    let user_agent = expected_req.headers().get("x-amz-user-agent").unwrap();
    assert_ua_contains_metric_values(user_agent, &["N"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
async fn test_observability_metrics_tracking() {
    use aws_smithy_observability::{
        instruments::{
            AsyncInstrumentBuilder, AsyncMeasure, Histogram, InstrumentBuilder, MonotonicCounter,
            ProvideInstrument, UpDownCounter,
        },
        meter::{Meter, ProvideMeter},
        Attributes, Context, TelemetryProvider,
    };
    use std::sync::Arc;

    // Create a test meter provider that is NOT a noop
    #[derive(Debug)]
    struct TestMeterProvider;

    impl ProvideMeter for TestMeterProvider {
        fn get_meter(&self, _scope: &'static str, _attributes: Option<&Attributes>) -> Meter {
            Meter::new(Arc::new(TestMeter))
        }
    }

    #[derive(Debug)]
    struct TestMeter;

    impl ProvideInstrument for TestMeter {
        fn create_gauge(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = f64>>, f64>,
        ) -> Arc<dyn AsyncMeasure<Value = f64>> {
            Arc::new(TestAsyncMeasure::<f64>::default())
        }

        fn create_up_down_counter(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn UpDownCounter>>,
        ) -> Arc<dyn UpDownCounter> {
            Arc::new(TestUpDownCounter)
        }

        fn create_async_up_down_counter(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = i64>>, i64>,
        ) -> Arc<dyn AsyncMeasure<Value = i64>> {
            Arc::new(TestAsyncMeasure::<i64>::default())
        }

        fn create_monotonic_counter(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn MonotonicCounter>>,
        ) -> Arc<dyn MonotonicCounter> {
            Arc::new(TestMonotonicCounter)
        }

        fn create_async_monotonic_counter(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = u64>>, u64>,
        ) -> Arc<dyn AsyncMeasure<Value = u64>> {
            Arc::new(TestAsyncMeasure::<u64>::default())
        }

        fn create_histogram(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn Histogram>>,
        ) -> Arc<dyn Histogram> {
            Arc::new(TestHistogram)
        }
    }

    #[derive(Debug, Default)]
    struct TestAsyncMeasure<T: Send + Sync + std::fmt::Debug>(std::marker::PhantomData<T>);

    impl<T: Send + Sync + std::fmt::Debug> AsyncMeasure for TestAsyncMeasure<T> {
        type Value = T;

        fn record(
            &self,
            _value: T,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }

        fn stop(&self) {}
    }

    #[derive(Debug)]
    struct TestUpDownCounter;

    impl UpDownCounter for TestUpDownCounter {
        fn add(
            &self,
            _value: i64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    #[derive(Debug)]
    struct TestMonotonicCounter;

    impl MonotonicCounter for TestMonotonicCounter {
        fn add(
            &self,
            _value: u64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    #[derive(Debug)]
    struct TestHistogram;

    impl Histogram for TestHistogram {
        fn record(
            &self,
            _value: f64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    // Set up the test meter provider as the global provider BEFORE creating the client
    let telemetry_provider = TelemetryProvider::builder()
        .meter_provider(Arc::new(TestMeterProvider))
        .build();

    // Set the global provider first
    aws_smithy_observability::global::set_telemetry_provider(telemetry_provider)
        .expect("failed to set telemetry provider");

    // Now create client - the interceptor will detect the non-noop provider
    let (http_client, captured_request) = capture_request(None);
    let config = Config::builder()
        .region(Region::new("us-east-1"))
        .http_client(http_client.clone())
        .with_test_defaults()
        .build();
    let client = Client::from_conf(config);

    let _ = client
        .put_object()
        .bucket("test-bucket")
        .key("test-key")
        .body(ByteStream::from_static("test data".as_bytes()))
        .send()
        .await;

    // Verify the User-Agent header contains m/5 (ObservabilityMetrics)
    let expected_req = captured_request.expect_request();
    let user_agent = expected_req.headers().get("x-amz-user-agent").unwrap();
    assert_ua_contains_metric_values(user_agent, &["5"]);

    // Clean up: reset to noop provider
    aws_smithy_observability::global::set_telemetry_provider(TelemetryProvider::noop())
        .expect("failed to reset telemetry provider");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
async fn test_otel_tracing_tracking() {
    use aws_smithy_observability::{
        instruments::{
            AsyncInstrumentBuilder, AsyncMeasure, Histogram, InstrumentBuilder, MonotonicCounter,
            ProvideInstrument, UpDownCounter,
        },
        meter::{Meter, ProvideMeter},
        Attributes, Context, TelemetryProvider,
    };
    use std::sync::Arc;

    // Create a test meter provider that is NOT a noop and is marked as OpenTelemetry
    #[derive(Debug)]
    struct TestOtelMeterProvider;

    impl ProvideMeter for TestOtelMeterProvider {
        fn get_meter(&self, _scope: &'static str, _attributes: Option<&Attributes>) -> Meter {
            Meter::new(Arc::new(TestOtelMeter))
        }
    }

    #[derive(Debug)]
    struct TestOtelMeter;

    impl ProvideInstrument for TestOtelMeter {
        fn create_gauge(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = f64>>, f64>,
        ) -> Arc<dyn AsyncMeasure<Value = f64>> {
            Arc::new(TestAsyncMeasure::<f64>::default())
        }

        fn create_up_down_counter(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn UpDownCounter>>,
        ) -> Arc<dyn UpDownCounter> {
            Arc::new(TestUpDownCounter)
        }

        fn create_async_up_down_counter(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = i64>>, i64>,
        ) -> Arc<dyn AsyncMeasure<Value = i64>> {
            Arc::new(TestAsyncMeasure::<i64>::default())
        }

        fn create_monotonic_counter(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn MonotonicCounter>>,
        ) -> Arc<dyn MonotonicCounter> {
            Arc::new(TestMonotonicCounter)
        }

        fn create_async_monotonic_counter(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = u64>>, u64>,
        ) -> Arc<dyn AsyncMeasure<Value = u64>> {
            Arc::new(TestAsyncMeasure::<u64>::default())
        }

        fn create_histogram(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn Histogram>>,
        ) -> Arc<dyn Histogram> {
            Arc::new(TestHistogram)
        }
    }

    #[derive(Debug, Default)]
    struct TestAsyncMeasure<T: Send + Sync + std::fmt::Debug>(std::marker::PhantomData<T>);

    impl<T: Send + Sync + std::fmt::Debug> AsyncMeasure for TestAsyncMeasure<T> {
        type Value = T;

        fn record(
            &self,
            _value: T,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }

        fn stop(&self) {}
    }

    #[derive(Debug)]
    struct TestUpDownCounter;

    impl UpDownCounter for TestUpDownCounter {
        fn add(
            &self,
            _value: i64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    #[derive(Debug)]
    struct TestMonotonicCounter;

    impl MonotonicCounter for TestMonotonicCounter {
        fn add(
            &self,
            _value: u64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    #[derive(Debug)]
    struct TestHistogram;

    impl Histogram for TestHistogram {
        fn record(
            &self,
            _value: f64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    // Set up the test meter provider as the global provider with OpenTelemetry flag
    let telemetry_provider = TelemetryProvider::builder()
        .meter_provider(Arc::new(TestOtelMeterProvider))
        .with_otel(true) // Mark as OpenTelemetry
        .build();

    // Set the global provider first
    aws_smithy_observability::global::set_telemetry_provider(telemetry_provider)
        .expect("failed to set telemetry provider");

    // Now create client - the plugin will detect the OpenTelemetry provider
    let (http_client, captured_request) = capture_request(None);
    let config = Config::builder()
        .region(Region::new("us-east-1"))
        .http_client(http_client.clone())
        .with_test_defaults()
        .build();
    let client = Client::from_conf(config);

    let _ = client
        .put_object()
        .bucket("test-bucket")
        .key("test-key")
        .body(ByteStream::from_static("test data".as_bytes()))
        .send()
        .await;

    // Verify the User-Agent header contains both m/4 (ObservabilityTracing) and m/6 (ObservabilityOtelTracing)
    // When OpenTelemetry is enabled, we should see:
    // - m/4: ObservabilityTracing (Smithy-level, indicates tracing is enabled)
    // - m/5: ObservabilityMetrics (AWS-level, indicates metrics are enabled)
    // - m/6: ObservabilityOtelTracing (AWS-level, indicates OpenTelemetry tracing)
    // - m/7: ObservabilityOtelMetrics (AWS-level, indicates OpenTelemetry metrics)
    let expected_req = captured_request.expect_request();
    let user_agent = expected_req.headers().get("x-amz-user-agent").unwrap();

    assert_ua_contains_metric_values(user_agent, &["4", "5", "6", "7"]);

    // Clean up: reset to noop provider
    aws_smithy_observability::global::set_telemetry_provider(TelemetryProvider::noop())
        .expect("failed to reset telemetry provider");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
async fn test_multiple_features_tracked() {
    use aws_smithy_observability::{
        instruments::{
            AsyncInstrumentBuilder, AsyncMeasure, Histogram, InstrumentBuilder, MonotonicCounter,
            ProvideInstrument, UpDownCounter,
        },
        meter::{Meter, ProvideMeter},
        Attributes, Context, TelemetryProvider,
    };
    use std::sync::Arc;

    // Create a test meter provider that is NOT a noop and is marked as OpenTelemetry
    #[derive(Debug)]
    struct TestOtelMeterProvider;

    impl ProvideMeter for TestOtelMeterProvider {
        fn get_meter(&self, _scope: &'static str, _attributes: Option<&Attributes>) -> Meter {
            Meter::new(Arc::new(TestOtelMeter))
        }
    }

    #[derive(Debug)]
    struct TestOtelMeter;

    impl ProvideInstrument for TestOtelMeter {
        fn create_gauge(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = f64>>, f64>,
        ) -> Arc<dyn AsyncMeasure<Value = f64>> {
            Arc::new(TestAsyncMeasure::<f64>::default())
        }

        fn create_up_down_counter(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn UpDownCounter>>,
        ) -> Arc<dyn UpDownCounter> {
            Arc::new(TestUpDownCounter)
        }

        fn create_async_up_down_counter(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = i64>>, i64>,
        ) -> Arc<dyn AsyncMeasure<Value = i64>> {
            Arc::new(TestAsyncMeasure::<i64>::default())
        }

        fn create_monotonic_counter(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn MonotonicCounter>>,
        ) -> Arc<dyn MonotonicCounter> {
            Arc::new(TestMonotonicCounter)
        }

        fn create_async_monotonic_counter(
            &self,
            _builder: AsyncInstrumentBuilder<'_, Arc<dyn AsyncMeasure<Value = u64>>, u64>,
        ) -> Arc<dyn AsyncMeasure<Value = u64>> {
            Arc::new(TestAsyncMeasure::<u64>::default())
        }

        fn create_histogram(
            &self,
            _builder: InstrumentBuilder<'_, Arc<dyn Histogram>>,
        ) -> Arc<dyn Histogram> {
            Arc::new(TestHistogram)
        }
    }

    #[derive(Debug, Default)]
    struct TestAsyncMeasure<T: Send + Sync + std::fmt::Debug>(std::marker::PhantomData<T>);

    impl<T: Send + Sync + std::fmt::Debug> AsyncMeasure for TestAsyncMeasure<T> {
        type Value = T;

        fn record(
            &self,
            _value: T,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }

        fn stop(&self) {}
    }

    #[derive(Debug)]
    struct TestUpDownCounter;

    impl UpDownCounter for TestUpDownCounter {
        fn add(
            &self,
            _value: i64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    #[derive(Debug)]
    struct TestMonotonicCounter;

    impl MonotonicCounter for TestMonotonicCounter {
        fn add(
            &self,
            _value: u64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    #[derive(Debug)]
    struct TestHistogram;

    impl Histogram for TestHistogram {
        fn record(
            &self,
            _value: f64,
            _attributes: Option<&Attributes>,
            _context: Option<&dyn Context>,
        ) {
        }
    }

    // Set up the test meter provider as the global provider with OpenTelemetry flag
    let telemetry_provider = TelemetryProvider::builder()
        .meter_provider(Arc::new(TestOtelMeterProvider))
        .with_otel(true) // Mark as OpenTelemetry
        .build();

    // Set the global provider first
    aws_smithy_observability::global::set_telemetry_provider(telemetry_provider)
        .expect("failed to set telemetry provider");

    // Now create client with BOTH custom endpoint AND OpenTelemetry
    let (http_client, captured_request) = capture_request(None);
    let config = Config::builder()
        .region(Region::new("us-east-1"))
        .http_client(http_client.clone())
        .endpoint_url("http://localhost:9000") // Custom endpoint -> metric N
        .with_test_defaults()
        .build();
    let client = Client::from_conf(config);

    let _ = client
        .put_object()
        .bucket("test-bucket")
        .key("test-key")
        .body(ByteStream::from_static("test data".as_bytes()))
        .send()
        .await;

    // Verify the User-Agent header contains all expected metrics:
    // - m/4: ObservabilityTracing (Smithy-level, indicates tracing is enabled)
    // - m/5: ObservabilityMetrics (AWS-level, indicates metrics are enabled)
    // - m/6: ObservabilityOtelTracing (AWS-level, indicates OpenTelemetry tracing)
    // - m/7: ObservabilityOtelMetrics (AWS-level, indicates OpenTelemetry metrics)
    // - m/N: EndpointOverride (AWS-level, indicates custom endpoint)
    let expected_req = captured_request.expect_request();
    let user_agent = expected_req.headers().get("x-amz-user-agent").unwrap();

    // All metrics should appear in the correct order
    assert_ua_contains_metric_values(user_agent, &["4", "5", "6", "7", "N"]);

    // Clean up: reset to noop provider
    aws_smithy_observability::global::set_telemetry_provider(TelemetryProvider::noop())
        .expect("failed to reset telemetry provider");
}

#[tokio::test]
async fn test_no_metrics_without_features() {
    let (http_client, captured_request) = capture_request(None);
    let config = Config::builder()
        .region(Region::new("us-east-1"))
        .http_client(http_client.clone())
        .with_test_defaults()
        .build();
    let client = Client::from_conf(config);

    let _ = client
        .put_object()
        .bucket("test-bucket")
        .key("test-key")
        .body(ByteStream::from_static("test data".as_bytes()))
        .send()
        .await;

    // Verify the User-Agent header does NOT contain new feature metrics
    let expected_req = captured_request.expect_request();
    let user_agent = expected_req.headers().get("x-amz-user-agent").unwrap();

    // Assert that none of the new feature metrics appear in the User-Agent header
    // New feature metrics: 1 (SsoLoginDevice), 2 (SsoLoginAuth), 4 (ObservabilityTracing),
    // 5 (ObservabilityMetrics), 6 (ObservabilityOtelTracing), 7 (ObservabilityOtelMetrics),
    // N (EndpointOverride)
    assert!(
        !user_agent.contains("m/1"),
        "User-Agent should not contain m/1 (SsoLoginDevice) without SSO: {}",
        user_agent
    );
    assert!(
        !user_agent.contains("m/2"),
        "User-Agent should not contain m/2 (SsoLoginAuth) without SSO: {}",
        user_agent
    );
    assert!(
        !user_agent.contains("m/4"),
        "User-Agent should not contain m/4 (ObservabilityTracing) without observability: {}",
        user_agent
    );
    assert!(
        !user_agent.contains("m/5"),
        "User-Agent should not contain m/5 (ObservabilityMetrics) without observability: {}",
        user_agent
    );
    assert!(
        !user_agent.contains("m/6"),
        "User-Agent should not contain m/6 (ObservabilityOtelTracing) without OpenTelemetry: {}",
        user_agent
    );
    assert!(
        !user_agent.contains("m/7"),
        "User-Agent should not contain m/7 (ObservabilityOtelMetrics) without OpenTelemetry: {}",
        user_agent
    );
    assert!(
        !user_agent.contains("m/N"),
        "User-Agent should not contain m/N (EndpointOverride) without custom endpoint: {}",
        user_agent
    );
}
