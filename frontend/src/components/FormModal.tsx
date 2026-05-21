import type { ComponentProps } from 'react';
import { Modal } from '@douyinfe/semi-ui';

const FORM_MODAL_WIDTHS = {
  medium: 720,
  large: 920,
  wide: 1120,
} as const;

type FormModalSize = keyof typeof FORM_MODAL_WIDTHS;

type FormModalProps = Omit<ComponentProps<typeof Modal>, 'footer' | 'width' | 'size'> & {
  size?: FormModalSize;
};

export function FormModal({ size = 'medium', className, style, bodyStyle, ...modalProps }: FormModalProps) {
  return (
    <Modal
      {...modalProps}
      className={['form-modal', className].filter(Boolean).join(' ')}
      width={FORM_MODAL_WIDTHS[size]}
      footer={null}
      style={{ maxWidth: 'calc(100vw - 32px)', ...style }}
      bodyStyle={{ maxHeight: 'calc(100vh - 220px)', overflowY: 'auto', ...bodyStyle }}
    />
  );
}
