# DAQ Decoder

A data processor that queries and decodes DAQ measurement signals. The JSON config contains information about the DAQ lists and the protocol used. 

Requires Kvaser CanKing 7.6 or later.

## Note
This service extension is meant to be automatically added and managed by the DAQ Configurator GUI extension for CanKing.

It's possible, but not recommended, to use this standalone with a saved or manually created JSON config. However, this usecase is not intended and may not work properly.