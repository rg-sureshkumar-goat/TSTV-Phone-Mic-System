#ifndef COPUS_SHIM_H
#define COPUS_SHIM_H

#include <opus.h>

/* Non-variadic wrappers around opus_encoder_ctl() so Swift can call them.
   `request` is one of the OPUS_SET_*_REQUEST / OPUS_GET_*_REQUEST constants. */
int copus_encoder_set_int(OpusEncoder *enc, int request, opus_int32 value);
int copus_encoder_get_int(OpusEncoder *enc, int request, opus_int32 *value);

#endif
