# DAQ Configurator

The DAQ Configurator is a customized workspace pane for setting up DAQ lists. These can either be used through CanKing with any compatible interface when connected to an ECU or saved as artefacts (DBC file and t-script file).

## Overview

This pane allows the user to read in an A2L file, filter & select through available measurement signals, assign to available events, optionally set up a t-script with trigger, and lastly, apply to a CanKing data-processor and download needed artefacts. When used in CanKing, it is best set up together with a Message Trace or Signal Plot. After starting a measurement the data-processor sends out CAN frames to the ECU to initiate DAQ with the provided config. The ECU will then send out frames with the requested data and interval. In the Message Trace the CAN frames will be tagged "DAQ" and to decode them the "Fixed Position" option in the top-left of the pane needs to be toggled. In the Signal Plot the signals need to be selected to show up in subsequent measurements.

## Support

Requires Kvaser CanKing 7.6 or later.

Currently, DAQ Configurator supports XCP and CCP as measurement protocols and CAN as transport protocol.

## Usage

The extension comes in the form of a 5-step wizard:

### - Load A2L

Select a .a2l file from your machine to start. Some general information about available protocols, signals and events will show. If the a2l supports several protocols one has to be picked and the extension will work toward the chosen protocol.

### - Select Signals

A scrollview with the available signals will show up which can be searched (by name, description or unit), filtered by measurement group (when the A2L declares groups; a group includes its sub-groups) and sorted by size. Virtual signals are marked as such and if a signal is not available for measurement through the extension then the reason will be shown.

### - Assign events

Assign each signal to any available event. One DAQ-list can only belong to one event.

### - Configure t-script

In this section different options related to the Memorator t-script can be set if that is the goal with this extension.

#### Options
- Override A2L bus settings - toggle to be able to manually set the bit rate to a different value.
- Trigger - select signal, operator and value. Several conditions are AND'ed together.
- Logging options - customize the CAN channel index, pretrigger time (ms) and if the logging should stop after a certain time.

It is also possible to avoid needing the DBC file when compiling the t-script. Checking "Standalone t-script (no DBC needed, experimental feature)" will embed the extra structs and decoders needed for the t-script to retrieve the correct values from the CAN frames.

### - Apply & export

Choose the source that the data-processor should latch on to and whether it should start DAQ with the given config on measurement start. Pressing "Apply to data processor" will compile the config, create a data-processor node and inject the config to it. After that, the option to download the DBC file, the t-script and the json config will be available.

#### DAQ list CAN identifiers

This step also lists every packed DAQ list together with the CAN identifier it transmits on:

- **Shared** - the list has no override and shares the transport's default id (`CAN_ID_SLAVE` for XCP, the DTO id for CCP), multiplexed by PID.
- **Fixed** - the A2L declares this list's id (XCP `DAQ_LIST_CAN_ID ... FIXED`, or CCP `CAN_ID_FIXED`). 

- An empty, editable field - the A2L declares this list's id as runtime-assignable. Enter a CAN id (hex with a `0x` prefix, or decimal) to have the extension set the id for that list as part of the init sequence; leave it blank to keep sharing the default id.

## Practical Information

- Make sure that the bus parameters for the CAN channel match the A2L or, if the ECU expects something different, try different baudrates until commands are responded to on the slave_id.

- The PID present in the config is only read from the A2L or an estimate and might not exist at all. The ECU responds with the correct information at runtime and the data processor adjusts accordingly, but the config and the generated artefacts will still have the old PID for each DAQ list.

- Setting the log level to "Debug" could reveal a lot more of what is happening in case something is not working correctly.