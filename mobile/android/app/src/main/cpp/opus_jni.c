#include <jni.h>
#include <opus.h>

JNIEXPORT jlong JNICALL
Java_org_tstvnews_phonemic_OpusEncoder_nativeCreate(JNIEnv *env, jclass cls, jint bitrate) {
    int error = 0;
    OpusEncoder *enc = opus_encoder_create(48000, 1, OPUS_APPLICATION_RESTRICTED_LOWDELAY, &error);
    if (error != OPUS_OK || enc == NULL) return 0;
    opus_encoder_ctl(enc, OPUS_SET_BITRATE(bitrate));
    opus_encoder_ctl(enc, OPUS_SET_COMPLEXITY(9));
    return (jlong)(intptr_t)enc;
}

JNIEXPORT jint JNICALL
Java_org_tstvnews_phonemic_OpusEncoder_nativeLookahead(JNIEnv *env, jclass cls, jlong handle) {
    opus_int32 samples = 0;
    opus_encoder_ctl((OpusEncoder *)(intptr_t)handle, OPUS_GET_LOOKAHEAD(&samples));
    return samples;
}

JNIEXPORT jint JNICALL
Java_org_tstvnews_phonemic_OpusEncoder_nativeEncode(JNIEnv *env, jclass cls, jlong handle, jfloatArray pcm, jbyteArray out) {
    jint capacity = (*env)->GetArrayLength(env, out);
    jfloat *in = (*env)->GetPrimitiveArrayCritical(env, pcm, NULL);
    jbyte *bytes = (*env)->GetPrimitiveArrayCritical(env, out, NULL);
    jint n = opus_encode_float((OpusEncoder *)(intptr_t)handle, in, 480, (unsigned char *)bytes, capacity);
    (*env)->ReleasePrimitiveArrayCritical(env, out, bytes, 0);
    (*env)->ReleasePrimitiveArrayCritical(env, pcm, in, JNI_ABORT);
    return n;
}

JNIEXPORT void JNICALL
Java_org_tstvnews_phonemic_OpusEncoder_nativeDestroy(JNIEnv *env, jclass cls, jlong handle) {
    opus_encoder_destroy((OpusEncoder *)(intptr_t)handle);
}
