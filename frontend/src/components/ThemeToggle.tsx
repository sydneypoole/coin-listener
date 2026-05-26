import { Select } from '@douyinfe/semi-ui';
import type { ThemeMode } from '../themeMode';

type ThemeToggleProps = {
  value: ThemeMode;
  onChange: (mode: ThemeMode) => void;
};

const options = [
  { label: '跟随系统', value: 'system' },
  { label: '浅色', value: 'light' },
  { label: '暗色', value: 'dark' },
];

export function ThemeToggle({ value, onChange }: ThemeToggleProps) {
  return (
    <Select
      value={value}
      optionList={options}
      aria-label="主题模式"
      size="small"
      style={{ width: 112 }}
      onChange={nextValue => onChange(nextValue as ThemeMode)}
    />
  );
}
