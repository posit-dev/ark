#!/usr/bin/env sh

set -eu

if [ "$(uname)" != "Darwin" ]; then
    exit 0
fi

# On macOS, we use install_name_tool to fix up the link to libR.dylib.
#
# Note that we still try to link with '-undefined dynamic_lookup', just to
# ensure that linking succeeds when we compile against a version of R compiled
# for a different architecture. This is mostly relevant when producing x86_64
# builds of ark on an arm64 machine.
#
# However, because using libR-sys still implies that the path to the R library
# ends up in the library load list, we have to modify that after the fact anyhow.

: ${AMALTHEA_BUILD_TYPE="debug"}

# Should be called from Amalthea root by default so that we can find the Ark executable
AMALTHEA_ROOT="${AMALTHEA_PATH:-.}"

# Normalize
AMALTHEA_ROOT=$(cd "$AMALTHEA_ROOT" && pwd)

# Get the path to the Ark executable
ARK_PATH="${AMALTHEA_ROOT}/target/${AMALTHEA_BUILD_TYPE}/ark"

if [ ! -f "$ARK_PATH" ]; then
    echo "Can't find Ark executable in $AMALTHEA_ROOT"
    echo "- Do you need to set 'AMALTHEA_ROOT'?"
    echo "- Do you need to run 'cargo build'?"
    exit 1
fi

# Figure out what version of R that we linked to
OLD_PATH=`otool -L target/debug/ark | grep libR.dylib | cut -c2- | cut -d' ' -f1`
NEW_PATH="@rpath/libR.dylib"

# Change that to use @rpath instead. We don't actually set an @rpath in the compiled
# executable (we inject R via DYLD_INSERT_LIBRARIES) so this is mainly just hygiene.
install_name_tool -change "${OLD_PATH}" "${NEW_PATH}" "${ARK_PATH}"
