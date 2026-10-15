import React from 'react';
import { Info } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from './ui/dialog';
import { Button } from './ui/button';
import type { UpdateInfo } from '@/services/updateService';
import { INTERNAL_UPDATE_POLICY } from '@/lib/update-policy';

interface UpdateDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  updateInfo: UpdateInfo | null;
}

export function UpdateDialog({ open, onOpenChange, updateInfo }: UpdateDialogProps) {
  // Ignore stale community update metadata: this build cannot safely install it.
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Info className="h-5 w-5 text-blue-600" />
            {INTERNAL_UPDATE_POLICY.title}
          </DialogTitle>
          <DialogDescription>{INTERNAL_UPDATE_POLICY.message}</DialogDescription>
        </DialogHeader>
        {updateInfo?.currentVersion && (
          <p className="py-4 text-sm text-muted-foreground">
            当前安装版本：{updateInfo.currentVersion}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>关闭</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
