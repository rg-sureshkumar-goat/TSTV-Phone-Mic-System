#include "copus_shim.h"

int copus_encoder_set_int(OpusEncoder *enc, int request, opus_int32 value) {
    return opus_encoder_ctl(enc, request, value);
}

int copus_encoder_get_int(OpusEncoder *enc, int request, opus_int32 *value) {
    return opus_encoder_ctl(enc, request, value);
}
