import type { UpdateInfo } from '@/services/updateService';

// Retain legacy entry points without advertising unverified community updates.
export function setUpdateDialogCallback(_callback: () => void) {}

export function showUpdateNotification(_updateInfo: UpdateInfo, _onUpdateClick?: () => void) {}
