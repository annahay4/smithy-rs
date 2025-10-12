/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

#![allow(dead_code)]

use aws_runtime::content_encoding::{
    header_value::AWS_CHUNKED, AwsChunkedBody, AwsChunkedBodyOptions, DeferredSigner,
};
use aws_smithy_checksums::http::HttpChecksum;
use aws_smithy_runtime_api::{
    box_error::BoxError,
    client::{
        interceptors::{
            context::{BeforeTransmitInterceptorContextMut, BeforeTransmitInterceptorContextRef},
            Intercept,
        },
        runtime_components::RuntimeComponents,
    },
};
use aws_smithy_types::{body::SdkBody, config_bag::ConfigBag, error::operation::BuildError};
use http_1x::HeaderValue;
use http_body_1x::Body;

#[derive(Debug)]
pub(crate) struct AwsChunkedContentEncodingInterceptor;

impl AwsChunkedContentEncodingInterceptor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Intercept for AwsChunkedContentEncodingInterceptor {
    fn name(&self) -> &'static str {
        "AwsChunkedContentEncodingInterceptor"
    }

    fn read_before_signing(
        &self,
        _context: &BeforeTransmitInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        // Make the following conditional, only when we need to sign chunks
        let (signer, sender) = DeferredSigner::new();
        cfg.interceptor_state().store_put(signer);
        cfg.interceptor_state().store_put(sender);
        Ok(())
    }

    fn modify_before_transmit(
        &self,
        ctx: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        if ctx.request().body().bytes().is_some() {
            // Not a streaming body, no need to apply aws-chunked encoding
            return Ok(());
        }

        let request = ctx.request_mut();

        let checksum_state = cfg
            .load::<crate::http_request_checksum::RequestChecksumInterceptorState>()
            .clone()
            .expect("state set");
        let checksum_algorithm = checksum_state.checksum_algorithm().expect("set");

        let original_body_size = request.body().size_hint().exact().ok_or_else(|| {
            BuildError::other(crate::http_request_checksum::Error::UnsizedRequestBody)
        })?;

        let mut body = {
            let body = std::mem::replace(request.body_mut(), SdkBody::taken());
            let signer = cfg
                .get_mut_from_interceptor_state::<DeferredSigner>()
                .unwrap();
            let signer = std::mem::replace(signer, DeferredSigner::empty());

            let checksum = checksum_algorithm.into_impl();
            let trailer_len = HttpChecksum::size(checksum.as_ref());
            let aws_chunked_body_options =
                AwsChunkedBodyOptions::new(original_body_size, vec![trailer_len]);
            let body = AwsChunkedBody::new(body, aws_chunked_body_options);
            let body = body.with_signer(signer);

            SdkBody::from_body_1_x(body)
        };

        std::mem::swap(request.body_mut(), &mut body);

        request.headers_mut().insert(
            http_1x::header::HeaderName::from_static("x-amz-decoded-content-length"),
            HeaderValue::from(original_body_size),
        );

        request.headers_mut().append(
            http_1x::header::CONTENT_ENCODING,
            HeaderValue::from_str(AWS_CHUNKED)
                .map_err(BuildError::other)
                .expect("\"aws-chunked\" will always be a valid HeaderValue"),
        );

        Ok(())
    }
}
