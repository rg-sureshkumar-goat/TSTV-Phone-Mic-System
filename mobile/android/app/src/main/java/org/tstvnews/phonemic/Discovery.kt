package org.tstvnews.phonemic

import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Handler
import android.os.Looper

/** Finds desktop apps advertising `_tstvmic._tcp` on the local network. */
class Discovery(private val nsd: NsdManager) {
    /** Called on the main thread with the current list. */
    var onChange: ((List<NsdServiceInfo>) -> Unit)? = null

    private val main = Handler(Looper.getMainLooper())
    private val found = linkedMapOf<String, NsdServiceInfo>()
    private var listener: NsdManager.DiscoveryListener? = null

    fun start() {
        if (listener != null) return
        val l = object : NsdManager.DiscoveryListener {
            override fun onServiceFound(info: NsdServiceInfo) = main.post { found[info.serviceName] = info; publish() }.let { }
            override fun onServiceLost(info: NsdServiceInfo) = main.post { found.remove(info.serviceName); publish() }.let { }
            override fun onDiscoveryStarted(serviceType: String) {}
            override fun onDiscoveryStopped(serviceType: String) {}
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) { listener = null }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
        }
        listener = l
        nsd.discoverServices(Wire.SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, l)
    }

    fun stop() {
        listener?.let { nsd.stopServiceDiscovery(it) }
        listener = null
        found.clear()
        publish()
    }

    private fun publish() {
        onChange?.invoke(found.values.sortedBy { it.serviceName })
    }
}
