import type { Update } from '@tauri-apps/plugin-updater';
import { getVersion } from '@tauri-apps/api/app';
import { INTERNAL_UPDATE_POLICY } from '@/lib/update-policy';

export interface UpdateInfo {
  available: boolean;
  currentVersion: string;
  status?: typeof INTERNAL_UPDATE_POLICY.status;
  version?: string;
  date?: string;
  body?: string;
  downloadUrl?: string;
}

export interface UpdateProgress {
  downloaded: number;
  total: number;
  percentage: number;
}

/** Internal builds only support manually installing a verified internal package. */
export class UpdateService {
  async checkForUpdates(_force = false): Promise<UpdateInfo> {
    return {
      available: false,
      currentVersion: await getVersion(),
      status: INTERNAL_UPDATE_POLICY.status,
    };
  }

  // Preserve the service boundary for existing callers, including stale Update objects.
  async downloadAndInstall(
    _update: Update,
    _onProgress?: (progress: UpdateProgress) => void,
  ): Promise<void> {
    throw new Error(INTERNAL_UPDATE_POLICY.message);
  }

  async getCurrentVersion(): Promise<string> {
    return getVersion();
  }

  wasCheckedRecently(): boolean {
    // No remote check is performed, so never claim there is a recent check.
    return false;
  }
}

export const updateService = new UpdateService();
