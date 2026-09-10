import {Box, Button, IconButton, MenuItem, Select, TextField, Typography} from "@mui/material";
import { TRIGGER_OPERATORS, TriggerCondition } from "./util";

export interface TriggerSectionProps {
  /** Signal names available for trigger conditions */
  signalNames: string[];
  conditions: TriggerCondition[];
  onConditionsChange: (value: TriggerCondition[]) => void;
}

/**
 * The signal trigger expression for the generated t-script. Conditions are
 * combined with AND and used as the start-logger trigger.
 */
function TriggerSection({
  signalNames,
  conditions,
  onConditionsChange,
}: TriggerSectionProps) {
  const updateCondition = (
    index: number,
    patch: Partial<TriggerCondition>,
  ) => {
    onConditionsChange(
      conditions.map((c, i) => (i === index ? { ...c, ...patch } : c)),
    );
  };

  return (
    <Box sx={{ display: "flex", flexDirection: "column", gap: 1 }}>
      <Typography variant="caption" color="text.secondary">
        Conditions are combined with AND (e.g. Rpm &gt; 2500). Used as the
        start-logger trigger in the generated t-script (Download t-script)
      </Typography>
      {conditions.map((condition, index) => (
        <Box key={index} sx={{ display: "flex", gap: 1, alignItems: "center" }}>
          <Select
            size="small"
            value={condition.signal}
            onChange={(e) => updateCondition(index, { signal: e.target.value })}
            displayEmpty
            sx={{ width: 260 }}
          >
            <MenuItem value="">
              <em>Select signal…</em>
            </MenuItem>
            {signalNames.map((name) => (
              <MenuItem key={name} value={name}>
                {name}
              </MenuItem>
            ))}
          </Select>
          <Select
            size="small"
            value={condition.operator}
            onChange={(e) =>
              updateCondition(index, {
                operator: e.target.value as TriggerCondition["operator"],
              })
            }
            sx={{ width: 90 }}
          >
            {TRIGGER_OPERATORS.map((op) => (
              <MenuItem key={op} value={op}>
                {op}
              </MenuItem>
            ))}
          </Select>
          <TextField
            size="small"
            label="Value"
            value={condition.value}
            onChange={(e) => updateCondition(index, { value: e.target.value })}
            sx={{ width: 140 }}
          />
          <IconButton
            size="small"
            aria-label="remove condition"
            onClick={() =>
              onConditionsChange(conditions.filter((_, i) => i !== index))
            }
          >
            ✕
          </IconButton>
        </Box>
      ))}
      <Box>
        <Button
          size="small"
          onClick={() =>
            onConditionsChange([
              ...conditions,
              { signal: "", operator: ">", value: "" },
            ])
          }
        >
          Add condition
        </Button>
      </Box>
    </Box>
  );
}

export default TriggerSection;
