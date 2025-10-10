/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use aws_smithy_runtime_api::{
    box_error::BoxError,
    client::{
        interceptors::{context::BeforeTransmitInterceptorContextMut, Intercept},
        runtime_components::RuntimeComponents,
    },
};
use aws_smithy_types::config_bag::ConfigBag;

#[derive(Debug)]
pub(crate) struct AwsChunkedContentEncodingInterceptor;

impl Intercept for AwsChunkedContentEncodingInterceptor {
    fn name(&self) -> &'static str {
        "AwsChunkedContentEncodingInterceptor"
    }

    fn modify_before_transmit(
        &self,
        ctx: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        Ok(())
    }
}
