/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

//! Endpoint override detection for business metrics tracking

use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_types::config_bag::ConfigBag;

use crate::sdk_feature::AwsSdkFeature;

/// Interceptor that detects when a custom endpoint URL is being used
/// and tracks it for business metrics.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct EndpointOverrideInterceptor;

impl EndpointOverrideInterceptor {
    /// Creates a new `EndpointOverrideInterceptor`
    pub fn new() -> Self {
        Self
    }
}

impl Intercept for EndpointOverrideInterceptor {
    fn name(&self) -> &'static str {
        "EndpointOverrideInterceptor"
    }

    fn read_before_execution(
        &self,
        _context: &aws_smithy_runtime_api::client::interceptors::context::BeforeSerializationInterceptorContextRef<'_>,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        // Check if a custom endpoint URL was configured
        // This is stored early in the config bag before endpoint resolution
        if let Some(endpoint_url) = cfg.load::<aws_types::endpoint_config::EndpointUrl>() {
            let url_str = endpoint_url.0.as_str();

            // Standard AWS endpoints follow patterns like:
            // - *.amazonaws.com
            // - *.amazonaws.com.cn (China)
            // If the endpoint doesn't match these patterns, it's a custom endpoint
            if !url_str.contains(".amazonaws.com") && !url_str.contains(".amazonaws.com.cn") {
                cfg.interceptor_state()
                    .store_append(AwsSdkFeature::EndpointOverride);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::client::interceptors::context::{Input, InterceptorContext};
    use aws_smithy_types::config_bag::ConfigBag;

    #[test]
    fn test_detects_custom_endpoint() {
        let context = InterceptorContext::new(Input::doesnt_matter());

        let mut cfg = ConfigBag::base();
        cfg.interceptor_state()
            .store_put(aws_types::endpoint_config::EndpointUrl(
                "https://custom.example.com".to_string(),
            ));

        let interceptor = EndpointOverrideInterceptor::new();
        let ctx = Into::into(&context);
        interceptor.read_before_execution(&ctx, &mut cfg).unwrap();

        let features: Vec<_> = cfg.load::<AwsSdkFeature>().collect();
        assert_eq!(features.len(), 1);
        assert!(features
            .iter()
            .any(|f| matches!(f, AwsSdkFeature::EndpointOverride)));
    }

    #[test]
    fn test_ignores_default_endpoint() {
        let context = InterceptorContext::new(Input::doesnt_matter());

        let mut cfg = ConfigBag::base();
        cfg.interceptor_state()
            .store_put(aws_types::endpoint_config::EndpointUrl(
                "https://service.amazonaws.com".to_string(),
            ));

        let interceptor = EndpointOverrideInterceptor::new();
        let ctx = Into::into(&context);
        interceptor.read_before_execution(&ctx, &mut cfg).unwrap();

        let features: Vec<_> = cfg.load::<AwsSdkFeature>().collect();
        assert_eq!(features.len(), 0);
    }

    #[test]
    fn test_no_endpoint_url_configured() {
        let context = InterceptorContext::new(Input::doesnt_matter());

        let mut cfg = ConfigBag::base();
        // No endpoint URL configured

        let interceptor = EndpointOverrideInterceptor::new();
        let ctx = Into::into(&context);
        interceptor.read_before_execution(&ctx, &mut cfg).unwrap();

        let features: Vec<_> = cfg.load::<AwsSdkFeature>().collect();
        assert_eq!(features.len(), 0);
    }
}
