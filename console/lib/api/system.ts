import type {
  MetricsHistory,
  Session,
  StorageStatus,
  StorageUsage,
  SystemInfo,
  SystemMetrics,
} from '@/types/api';

import { request } from './client';
import { ApiError } from './error';

/**
 * Reads deployment mode and capabilities.
 *
 * The backend is authoritative: the console never infers what a deployment can
 * do from its own environment.
 */
export function fetchSystemInfo(signal?: AbortSignal): Promise<SystemInfo> {
  return request<SystemInfo>('/v1/system/info', signal ? { signal } : {});
}

/** Reads the identity behind the current session. */
export function fetchSession(signal?: AbortSignal): Promise<Session> {
  return request<Session>('/v1/auth/session', signal ? { signal } : {});
}

export function fetchStorageUsage(signal?: AbortSignal): Promise<StorageUsage> {
  return request<StorageUsage>('/v1/storage/usage', signal ? { signal } : {});
}

export function fetchStorageStatus(signal?: AbortSignal): Promise<StorageStatus> {
  return request<StorageStatus>('/v1/storage/status', signal ? { signal } : {});
}

/**
 * Reads the server's recent counter readings.
 *
 * Used once, to seed the charts. A failure here is not worth surfacing: the
 * screen still works by taking its own readings, it just starts empty, which is
 * exactly what it did before this existed.
 */
export function fetchSystemMetricsHistory(signal?: AbortSignal): Promise<MetricsHistory> {
  return request<MetricsHistory>('/v1/system/metrics/history', signal ? { signal } : {});
}

/** Reads current metric values through the management plane. */
export async function fetchSystemMetrics(signal?: AbortSignal): Promise<SystemMetrics> {
  const timeout = AbortSignal.timeout(10_000);
  try {
    return await request<SystemMetrics>('/v1/system/metrics', {
      signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
    });
  } catch (error) {
    if (timeout.aborted && !signal?.aborted) {
      throw new ApiError({
        status: 503,
        code: 'METRICS_TIMEOUT',
        message: 'Metrics took too long to respond. Try refreshing again.',
        requestId: null,
      });
    }
    throw error;
  }
}
