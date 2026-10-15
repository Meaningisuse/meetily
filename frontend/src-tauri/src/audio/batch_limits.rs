use anyhow::{anyhow, Result};
use std::sync::{Arc, LazyLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) fn check_source_file(path: &std::path::Path) -> Result<()> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > 20 * 1024 * 1024 * 1024 {
        return Err(anyhow!("Audio source must be a file no larger than 20 GiB"));
    }
    Ok(())
}

/// Application-owned PCM budget, not a process RSS limit. Two copies of the
/// source and target cover growth, conversion and VAD; fixed scratch covers
/// packet copies, one sinc window, VAD state and segment splitting overlap.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BatchAudioLimits {
    pub working_bytes: usize,
    pub scratch_bytes: usize,
    pub packet_bytes: usize,
    pub temp_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("Audio exceeds the local batch memory budget. Trim the recording or reduce its sample rate/channels before importing or retranscribing.")]
pub(crate) struct AudioBudgetExceeded;

impl Default for BatchAudioLimits {
    fn default() -> Self {
        Self {
            working_bytes: 2 * 1024 * 1024 * 1024,
            scratch_bytes: 256 * 1024 * 1024,
            packet_bytes: 16 * 1024 * 1024,
            temp_bytes: 512 * 1024 * 1024,
        }
    }
}

impl BatchAudioLimits {
    pub fn check_format(&self, rate: u32, channels: u16) -> Result<()> {
        if rate == 0 || rate > 192_000 || channels == 0 || channels > 32 {
            return Err(anyhow!(
                "Unsupported decoded audio format (sample rate or channel count)"
            ));
        }
        Ok(())
    }

    pub fn target_samples(&self, samples: usize, rate: u32, channels: u16) -> Result<usize> {
        self.check_format(rate, channels)?;
        let denominator = rate as u128 * channels as u128;
        let target = (samples as u128 * 16_000).div_ceil(denominator);
        usize::try_from(target).map_err(|_| AudioBudgetExceeded.into())
    }

    pub fn check_source(&self, samples: usize, rate: u32, channels: u16) -> Result<()> {
        let target = self.target_samples(samples, rate, channels)?;
        let bytes = (samples as u128 + target as u128) * 8 + self.scratch_bytes as u128;
        if bytes > self.working_bytes as u128 {
            return Err(AudioBudgetExceeded.into());
        }
        Ok(())
    }

    /// Bound capacity as well as length. Computing a format-specific maximum
    /// lets Vec grow geometrically without allowing its capacity past the budget.
    pub fn max_source_samples(&self, rate: u32, channels: u16) -> Result<usize> {
        self.check_format(rate, channels)?;
        let mut low = 0;
        let mut high =
            self.working_bytes.saturating_sub(self.scratch_bytes) / 8 / channels as usize;
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            if self
                .check_source(mid * channels as usize, rate, channels)
                .is_ok()
            {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        Ok(low * channels as usize)
    }

    pub fn check_packet(&self, frames: usize, channels: u16) -> Result<()> {
        if frames as u128 * channels as u128 * 4 > self.packet_bytes as u128 {
            return Err(AudioBudgetExceeded.into());
        }
        Ok(())
    }

    pub fn check_vad_capacity(&self, input: usize, output: usize) -> Result<()> {
        if (input as u128 + output as u128) * 4 + self.scratch_bytes as u128
            > self.working_bytes as u128
        {
            return Err(AudioBudgetExceeded.into());
        }
        Ok(())
    }

    pub fn max_target_capacity(&self, input_capacity: usize) -> Result<usize> {
        let input_bytes = input_capacity.checked_mul(4).ok_or(AudioBudgetExceeded)?;
        let available = self
            .working_bytes
            .checked_sub(self.scratch_bytes)
            .and_then(|n| n.checked_sub(input_bytes))
            .ok_or(AudioBudgetExceeded)?;
        Ok(available / 8)
    }
}

pub(crate) fn reserve_samples(
    buffer: &mut Vec<f32>,
    additional: usize,
    maximum: usize,
) -> Result<()> {
    let required = buffer
        .len()
        .checked_add(additional)
        .ok_or(AudioBudgetExceeded)?;
    if required > maximum {
        return Err(AudioBudgetExceeded.into());
    }
    if buffer.capacity() < required {
        let capacity = buffer
            .capacity()
            .max(4096)
            .saturating_mul(2)
            .min(maximum)
            .max(required);
        buffer
            .try_reserve_exact(capacity - buffer.len())
            .map_err(|_| AudioBudgetExceeded)?;
    }
    if buffer.capacity() > maximum {
        return Err(AudioBudgetExceeded.into());
    }
    Ok(())
}

static BATCH_SLOT: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));

/// Shared across import, retranscription and metadata's decode fallback.
/// Clones travel with blocking work/results so cancelling an async waiter does
/// not release admission while the detached blocking task still owns PCM.
#[derive(Clone, Debug)]
pub(crate) struct BatchAudioLease {
    _permit: Arc<OwnedSemaphorePermit>,
}

impl BatchAudioLease {
    pub fn try_acquire() -> Result<Self> {
        Self::from_slot(BATCH_SLOT.clone())
    }

    fn from_slot(slot: Arc<Semaphore>) -> Result<Self> {
        slot.try_acquire_owned().map(|p| Self { _permit: Arc::new(p) }).map_err(|_| {
            anyhow!("Another audio import, retranscription or metadata decode is in progress. Retry after it finishes.")
        })
    }

    pub fn spawn_blocking<T, F>(&self, work: F) -> tokio::task::JoinHandle<(T, Self)>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let lease = self.clone();
        tokio::task::spawn_blocking(move || (work(), lease))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_checks_exact_boundary_upsampling_and_overflow_without_allocating() {
        let limits = BatchAudioLimits {
            working_bytes: 224,
            scratch_bytes: 64,
            ..Default::default()
        };
        assert!(limits.check_source(10, 16_000, 1).is_ok());
        assert!(limits.check_source(11, 16_000, 1).is_err());
        assert!(limits.check_source(10, 8_000, 1).is_err());
        assert!(limits.check_source(usize::MAX, 1, 1).is_err());
        assert!(limits.check_source(1, 0, 1).is_err());
        assert!(limits.check_source(1, 16_000, 0).is_err());
        assert_eq!(limits.max_source_samples(16_000, 1).unwrap(), 10);
        assert!(BatchAudioLimits::default()
            .check_source(120 * 60 * 16_000, 16_000, 1)
            .is_ok());
    }

    #[test]
    fn capacity_reservation_rejects_growth_before_allocation() {
        let mut samples = vec![0.0; 8];
        let pointer = samples.as_ptr();
        assert!(reserve_samples(&mut samples, 3, 10).is_err());
        assert_eq!(samples.len(), 8);
        assert_eq!(samples.as_ptr(), pointer);
        reserve_samples(&mut samples, 2, 10).unwrap();
        assert!(samples.capacity() <= 10);
        let limits = BatchAudioLimits {
            packet_bytes: 32,
            ..Default::default()
        };
        assert!(limits.check_packet(4, 2).is_ok());
        assert!(limits.check_packet(5, 2).is_err());
    }

    #[tokio::test]
    async fn completed_unclaimed_pcm_result_keeps_its_lease() {
        let slot = Arc::new(Semaphore::new(1));
        let lease = BatchAudioLease::from_slot(slot.clone()).unwrap();
        let work = lease.spawn_blocking(|| vec![0.0f32; 32]);
        drop(lease);
        let result = work.await.unwrap();
        assert!(BatchAudioLease::from_slot(slot.clone()).is_err());
        drop(result);
        assert!(BatchAudioLease::from_slot(slot).is_ok());
    }

    #[tokio::test]
    async fn lease_remains_busy_after_async_waiter_is_cancelled() {
        let slot = Arc::new(Semaphore::new(1));
        let lease = BatchAudioLease::from_slot(slot.clone()).unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let work = lease.spawn_blocking(move || {
            started_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            vec![0.0f32; 32]
        });
        let waiter = tokio::spawn(async move { work.await.unwrap() });
        started_rx.await.unwrap();
        drop(lease);
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(BatchAudioLease::from_slot(slot.clone()).is_err());
        finish_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while BatchAudioLease::from_slot(slot.clone()).is_err() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
