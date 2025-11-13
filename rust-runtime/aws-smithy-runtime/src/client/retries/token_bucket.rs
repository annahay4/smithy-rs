/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use aws_smithy_types::config_bag::{Storable, StoreReplace};
use aws_smithy_types::retry::ErrorKind;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::trace;

const DEFAULT_CAPACITY: usize = 500;
const DEFAULT_RETRY_COST: u32 = 5;
const DEFAULT_RETRY_TIMEOUT_COST: u32 = DEFAULT_RETRY_COST * 2;
const PERMIT_REGENERATION_AMOUNT: usize = 1;
const DEFAULT_SUCCESS_REWARD: f64 = 0.0;

/// Token bucket used for standard and adaptive retry.
#[derive(Clone, Debug)]
pub struct TokenBucket {
    semaphore: Arc<Semaphore>,
    max_permits: usize,
    timeout_retry_cost: u32,
    retry_cost: u32,
    success_reward: f64,
    fractional_tokens: Arc<Mutex<f64>>,
}

impl Storable for TokenBucket {
    type Storer = StoreReplace<Self>;
}

impl Default for TokenBucket {
    fn default() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(DEFAULT_CAPACITY)),
            max_permits: DEFAULT_CAPACITY,
            timeout_retry_cost: DEFAULT_RETRY_TIMEOUT_COST,
            retry_cost: DEFAULT_RETRY_COST,
            success_reward: DEFAULT_SUCCESS_REWARD,
            fractional_tokens: Arc::new(Mutex::new(0.0)),
        }
    }
}

impl TokenBucket {
    /// Creates a new `TokenBucket` with the given initial quota.
    pub fn new(initial_quota: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(initial_quota)),
            max_permits: initial_quota,
            ..Default::default()
        }
    }

    /// A token bucket with unlimited capacity that allows retries at no cost.
    pub fn unlimited() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(Semaphore::MAX_PERMITS)),
            max_permits: Semaphore::MAX_PERMITS,
            timeout_retry_cost: 0,
            retry_cost: 0,
            success_reward: 0.0,
            fractional_tokens: Arc::new(Mutex::new(0.0)),
        }
    }

    /// Creates a builder for constructing a `TokenBucket`.
    pub fn builder() -> TokenBucketBuilder {
        TokenBucketBuilder::default()
    }

    pub(crate) fn acquire(&self, err: &ErrorKind) -> Option<OwnedSemaphorePermit> {
        let retry_cost = if err == &ErrorKind::TransientError {
            self.timeout_retry_cost
        } else {
            self.retry_cost
        };

        self.semaphore
            .clone()
            .try_acquire_many_owned(retry_cost)
            .ok()
    }

    pub(crate) fn regenerate_a_token(&self) {
        self.add_tokens(PERMIT_REGENERATION_AMOUNT);
    }

    pub(crate) fn reward_success(&self) {
        if self.success_reward > 0.0 {
            *self.fractional_tokens.lock().unwrap() += self.success_reward;
        }

        let full_tokens_accumulated = self.fractional_tokens.lock().unwrap().floor();
        if full_tokens_accumulated >= 1.0 {
            *self.fractional_tokens.lock().unwrap() -= full_tokens_accumulated;
            self.add_tokens(full_tokens_accumulated as usize);
        }
    } 

    fn add_tokens(&self, amount: usize) {
        let tokens_to_add = amount.min(self.max_permits - self.semaphore.available_permits());
        trace!("adding {tokens_to_add} back into the bucket");
        self.semaphore.add_permits(tokens_to_add);
    }

    #[cfg(all(test, any(feature = "test-util", feature = "legacy-test-util")))]
    pub(crate) fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// Builder for constructing a `TokenBucket`.
#[derive(Clone, Debug, Default)]
pub struct TokenBucketBuilder {
    capacity: Option<usize>,
    retry_cost: Option<u32>,
    timeout_retry_cost: Option<u32>,
    success_reward: Option<f64>,
}

impl TokenBucketBuilder {
    /// Creates a new `TokenBucketBuilder` with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum bucket capacity for the builder.
    pub fn capacity(mut self, capacity: usize) -> Self {
        self.capacity = Some(capacity);
        self
    }

    /// Sets the specified retry cost for the builder.
    pub fn retry_cost(mut self, retry_cost: u32) -> Self {
        self.retry_cost = Some(retry_cost);
        self
    }

    /// Sets the specified timeout retry cost for the builder.
    pub fn timeout_retry_cost(mut self, timeout_retry_cost: u32) -> Self {
        self.timeout_retry_cost = Some(timeout_retry_cost);
        self
    }

    /// Sets the reward for any successful request for the builder.
    pub fn success_reward(mut self, reward: f64) -> Self {
        self.success_reward = Some(reward);
        self
    }

    /// Builds a `TokenBucket`.
    pub fn build(self) -> TokenBucket {
        TokenBucket {
            semaphore: Arc::new(Semaphore::new(self.capacity.unwrap_or(DEFAULT_CAPACITY))),
            max_permits: self.capacity.unwrap_or(DEFAULT_CAPACITY),
            retry_cost: self.retry_cost.unwrap_or(DEFAULT_RETRY_COST),
            timeout_retry_cost: self.timeout_retry_cost.unwrap_or(DEFAULT_RETRY_TIMEOUT_COST),
            success_reward: self.success_reward.unwrap_or(DEFAULT_SUCCESS_REWARD),
            fractional_tokens: Arc::new(Mutex::new(0.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unlimited_token_bucket() {
        let bucket = TokenBucket::unlimited();

        // Should always acquire permits regardless of error type
        assert!(bucket.acquire(&ErrorKind::ThrottlingError).is_some());
        assert!(bucket.acquire(&ErrorKind::TransientError).is_some());

        // Should have maximum capacity
        assert_eq!(bucket.max_permits, Semaphore::MAX_PERMITS);

        // Should have zero retry costs
        assert_eq!(bucket.retry_cost, 0);
        assert_eq!(bucket.timeout_retry_cost, 0);

        // The loop count is arbitrary; should obtain permits without limit
        let mut permits = Vec::new();
        for _ in 0..100 {
            let permit = bucket.acquire(&ErrorKind::ThrottlingError);
            assert!(permit.is_some());
            permits.push(permit);
            // Available permits should stay constant
            assert_eq!(
                tokio::sync::Semaphore::MAX_PERMITS,
                bucket.semaphore.available_permits()
            );
        }
    }

    #[test]
    fn test_bounded_permits_exhaustion() {
        let bucket = TokenBucket::new(10);
        let mut permits = Vec::new();

        for _ in 0..100 {
            let permit = bucket.acquire(&ErrorKind::ThrottlingError);
            if let Some(p) = permit {
                permits.push(p);
            } else {
                break;
            }
        }

        assert_eq!(permits.len(), 2); // 10 capacity / 5 retry cost = 2 permits

        // Verify next acquisition fails
        assert!(bucket.acquire(&ErrorKind::ThrottlingError).is_none());
    }

    #[test]
    fn test_fractional_tokens_accumulate_and_convert() {
        let bucket = TokenBucket::builder()
            .capacity(10)
            .success_reward(0.4)
            .build();
        
        // acquire 10 tokens to bring capacity below max so we can test accumulation
        let _hold_permit = bucket.acquire(&ErrorKind::TransientError);
        assert_eq!(bucket.semaphore.available_permits(), 0);

        // First success: 0.4 fractional tokens
        bucket.reward_success();
        assert_eq!(bucket.semaphore.available_permits(), 0);
        
        // Second success: 0.8 fractional tokens
        bucket.reward_success();
        assert_eq!(bucket.semaphore.available_permits(), 0);
        
        // Third success: 1.2 fractional tokens -> 1 full token added
        bucket.reward_success();
        assert_eq!(bucket.semaphore.available_permits(), 1);
    }

    #[test]
    fn test_fractional_tokens_respect_max_capacity() {
        let bucket = TokenBucket::builder()
            .capacity(10)
            .success_reward(2.0)
            .build();
        
        for _ in 0..20 {
            bucket.reward_success();
        }
        
        assert!(bucket.semaphore.available_permits() == 10);
    }

    #[cfg(any(feature = "test-util", feature = "legacy-test-util"))]
    #[test]
    fn test_builder_with_custom_values() {
        let bucket = TokenBucket::builder()
            .capacity(100)
            .retry_cost(10)
            .timeout_retry_cost(20)
            .success_reward(0.5)
            .build();
        
        assert_eq!(bucket.max_permits, 100);
        assert_eq!(bucket.retry_cost, 10);
        assert_eq!(bucket.timeout_retry_cost, 20);
        assert_eq!(bucket.success_reward, 0.5);
    }

}
