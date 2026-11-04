package org.tstvnews.phonemic

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.LaunchedEffect
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning

class MainActivity : ComponentActivity() {
    private val model by lazy { (application as TstvApp).model }
    private var afterPermissions: (() -> Unit)? = null

    private val permissionRequest = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { results ->
        val action = afterPermissions
        afterPermissions = null
        if (results[Manifest.permission.RECORD_AUDIO] == true || hasMic()) {
            action?.invoke()
        } else {
            model.errorMessage = "Microphone access is off. Turn it on in Settings › Apps › TSTV Mic › Permissions."
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        model.withPermissions = ::withPermissions
        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                LaunchedEffect(model.phase) {
                    // Keep the screen awake while connected.
                    if (model.phase == AppModel.Phase.Idle) {
                        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    } else {
                        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    }
                }
                AppScreens(model, onScan = ::scan)
            }
        }
        handleLink(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleLink(intent)
    }

    private fun handleLink(intent: Intent?) {
        if (intent?.action == Intent.ACTION_VIEW) intent.dataString?.let { model.openLink(it) }
    }

    private fun hasMic() = checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    private fun withPermissions(action: () -> Unit) {
        val wanted = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= 31) add(Manifest.permission.BLUETOOTH_CONNECT)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (wanted.isEmpty()) {
            action()
        } else {
            afterPermissions = action
            permissionRequest.launch(wanted.toTypedArray())
        }
    }

    private fun scan() {
        val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
        GmsBarcodeScanning.getClient(this, options).startScan()
            .addOnSuccessListener { code -> code.rawValue?.let { model.openLink(it) } }
            .addOnFailureListener { model.errorMessage = "The QR scanner isn't available on this phone. Use your camera app, or type the address." }
    }
}
