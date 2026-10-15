import { useCallback, useState } from 'react';
import { updateService, UpdateInfo } from '@/services/updateService';

interface UseUpdateCheckOptions {
  checkOnMount?: boolean;
  showNotification?: boolean;
  onUpdateAvailable?: (info: UpdateInfo) => void;
}

export function useUpdateCheck(_options: UseUpdateCheckOptions = {}) {
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [isChecking, setIsChecking] = useState(false);

  // Explicit requests read the installed version and policy. Startup never schedules a check.
  const checkForUpdates = useCallback(async (force = false) => {
    setIsChecking(true);
    try {
      setUpdateInfo(await updateService.checkForUpdates(force));
    } catch (error) {
      console.error('Failed to read installed app version:', error);
    } finally {
      setIsChecking(false);
    }
  }, []);

  return { updateInfo, isChecking, checkForUpdates };
}
