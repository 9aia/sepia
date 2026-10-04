import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../ui/select";

interface FilterOption {
  readonly value: string;
  readonly label: string;
}

interface FilterSelectProps {
  readonly label: string;
  readonly value: string;
  readonly options: ReadonlyArray<FilterOption>;
  onChange: (value: string) => void;
}

export function FilterSelect({ label, value, options, onChange }: FilterSelectProps) {
  return (
    <Select
      value={value}
      onValueChange={(next) => {
        if (next !== null) onChange(next);
      }}
    >
      <SelectTrigger aria-label={label} className="h-7 flex-1 text-xs">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map((option) => (
          <SelectItem key={option.value} value={option.value}>
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
