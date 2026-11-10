# DAQ Configurator

A CanKing workspace view that sets up DAQ measurements from an A2L file. The result runs in CanKing, with any compatible
Kvaser interface connected to the ECU, or as files: a DBC and a t-script for a Memorator logger.

Requires Kvaser CanKing 7.6 or later. Supports XCP on CAN and CAN FD, and CCP on CAN.

## Overview

Load an A2L, select signals, assign them to events, optionally configure a t-script, then apply the setup to a DAQ Decoder data processor and download the files.
Once a measurement starts, the data processor sends the DAQ commands to the ECU and the ECU streams the selected signals back.

Use this extension with a Message Trace or a Signal Plot. In the Message Trace the DAQ frames are named "DAQ"; turn on "Fixed Position" (top left of the trace) to see them decoded. 
In the Signal Plot, add the signals to be tracked and they appear in later measurements.

## The five steps

### Load A2L

Pick an `.a2l` file. The view shows the protocols, signal and event counts and the bus settings it found. If the file declares both XCP and CCP, choose one of them.

A warning lists any keywords in the file that change the results but that the configurator does not read, for example `/include` or a relative PID field. Outputs that depend on them may be wrong.

### Select signals

Search by name, description or unit, filter by measurement group (a group includes its sub-groups), and sort by size.
Virtual signals are marked. A signal that can't be measured shows why.

### Assign events

Assign each signal to one of the ECU's events, or keep the event the A2L gives it. Each DAQ list belongs to one event.

### Configure t-script

Options for the downloadable t-script:

- **CAN bus**: override the A2L's bit rate and CAN FD settings.
- **Trigger**: signal, operator and value. Several conditions must all hold.
- **Stop**: conditions in the same form, and a **Stop frame CAN id**. When the conditions hold or a frame with that id arrives, the script stops logging and stops DAQ on the ECU for good.
- **Logging**: CAN channel, pretrigger time, and **Stop logging after** a time.
- **Captures** (with a stop time, default 1): after each capture the script waits until the trigger condition has been false, and starts the next capture when it turns true again.
  DAQ keeps running in between. After the last capture the script stops DAQ on the ECU. Leave it blank for no limit.
- **Restart DAQ after silence**: the script checks once per this period that DAQ frames still arrive. If none did, for example because the ECU restarted, it sets up DAQ again 5 s later: one to two periods plus 5 s after the last frame.
  Leave it blank to never do that. A DAQ setup the ECU doesn't answer always starts over after 5 seconds.
- **Standalone t-script**: the script decodes the signals itself, so it compiles without the DBC.

DAQ on the ECU only stops when the script tells it to: after the last capture, on the stop condition or stop frame, or when the script is stopped. 
If the logger loses power or is unplugged while DAQ runs, the ECU keeps sending DAQ frames until something else stops it or it restarts, much like a car left running.

### Apply & export

Choose the source the data processor reads from, and whether it starts DAQ on the ECU when a measurement starts.
**Apply to data processor** creates the data processor, or updates the one created before. Then the DBC, the t-script and the JSON config can be downloaded.

The **DAQ lists** table shows each list, its event and its CAN id:

- **Shared**: the list uses the default DAQ id (`CAN_ID_SLAVE` for XCP, the DTO id for CCP), multiplexed by PID.
- **Fixed**: the A2L fixes the list's id (XCP `DAQ_LIST_CAN_ID ... FIXED`, CCP `CAN_ID_FIXED`).
- **A text field**: the A2L lets the id be assigned. Enter one (decimal, or hex with `0x`) and the data processor sets it when it starts DAQ; leave it blank to share the default id.

A **Prescaler** column appears when the ECU supports it. A list with prescaler n is sent on every n-th cycle of its event. Blank means 1.

## Practical information

- The CAN channel's bus settings must match the ECU. When the A2L is wrong, try other bit rates until the ECU answers.
- The PIDs in the config and the files come from the A2L or are estimated. The data processor corrects them from the ECU's replies while it runs, but the downloaded files keep the original values.
- Set CanKing's log level to Debug to see more of what the data processor does.
