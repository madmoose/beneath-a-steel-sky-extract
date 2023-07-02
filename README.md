# Beneath a Steel Sky Extract

This is a work-in-progress program to extract resources from 
Revolution Software's 1994 game Beneath a Steel Sky.

The program will create a directory called `dump` into which the 
extracted resources will be placed.

There's no great way to tell whether resources are images, palettes, 
audio, or other, so a very crude guess is for now. Resources types 
will be specified in an external file once the types have been 
determined.

So far it's only been tested with the freeware release `bass-cd-1.2` 
which you can get from https://scummvm.org/

```
Extracts and decodes data files from Beneath a Steel Sky

Usage: beneath-a-steel-sky-extract [OPTIONS] <PATH>

Arguments:
  <PATH>  Path to game data files

Options:
  -d, --dump-csv  Dump the resource list to `resource.csv`
  -h, --help      Print help
```
