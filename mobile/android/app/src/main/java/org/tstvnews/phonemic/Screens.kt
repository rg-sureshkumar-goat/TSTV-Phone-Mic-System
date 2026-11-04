package org.tstvnews.phonemic

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private val LiveGreen = Color(0xFF1F9E4F)
private val MutedRed = Color(0xFFD1142B)
private val Orange = Color(0xFFE08600)

@Composable
fun AppScreens(model: AppModel, onScan: () -> Unit) {
    var showSettings by remember { mutableStateOf(false) }
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Box(Modifier.safeDrawingPadding()) {
            when {
                showSettings -> SettingsScreen(model, onDone = { showSettings = false })
                model.phase == AppModel.Phase.Idle -> ConnectScreen(model, onScan, onSettings = { showSettings = true })
                else -> LiveScreen(model)
            }
        }
    }
}

@Composable
private fun ConnectScreen(model: AppModel, onScan: () -> Unit, onSettings: () -> Unit) {
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("TSTV Phone Mic", fontSize = 30.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            IconButton(onClick = onSettings) { Icon(Icons.Filled.Settings, contentDescription = "Settings") }
        }
        OutlinedTextField(
            value = model.name, onValueChange = model::updateName, singleLine = true,
            label = { Text("Your name") }, modifier = Modifier.fillMaxWidth(),
        )

        model.pendingTarget?.let { pending ->
            Text("Join ${pending.label}", fontWeight = FontWeight.SemiBold)
            if (model.needsCode) {
                OutlinedTextField(
                    value = model.joinCode, onValueChange = model::updateJoinCode, singleLine = true,
                    label = { Text("Join code") }, modifier = Modifier.fillMaxWidth(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                )
            }
            ErrorText(model.errorMessage)
            Button(
                onClick = model::connectToPending,
                enabled = model.canConnect && (!model.needsCode || model.joinCode.isNotEmpty()),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Connect") }
        }

        Button(onClick = onScan, modifier = Modifier.fillMaxWidth()) { Text("Scan QR code") }
        Hint("On the desktop app, click Connect a phone to show the code.")

        Text("Desktops on this network", fontWeight = FontWeight.SemiBold)
        if (model.desktops.isEmpty()) Hint("Looking for the desktop app…")
        model.desktops.forEach { desktop ->
            OutlinedButton(
                onClick = { model.connect(DesktopTarget.Service(desktop)) },
                enabled = model.canConnect,
                modifier = Modifier.fillMaxWidth(),
            ) { Text(desktop.serviceName, maxLines = 1, overflow = TextOverflow.Ellipsis) }
        }

        Text("Or type the desktop's address", fontWeight = FontWeight.SemiBold)
        OutlinedTextField(
            value = model.manualHost, onValueChange = model::updateManualHost, singleLine = true,
            label = { Text("e.g. 192.168.0.100") }, modifier = Modifier.fillMaxWidth(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
        )
        OutlinedButton(
            onClick = model::connectToManualHost,
            enabled = model.canConnect && model.manualHost.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Connect") }
        Hint("It's shown at the top of the desktop app. Use this if the desktop doesn't appear above (some networks block discovery).")

        if (model.pendingTarget == null) ErrorText(model.errorMessage)
    }
}

@Composable
private fun LiveScreen(model: AppModel) {
    var confirmDisconnect by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(verticalAlignment = Alignment.Top) {
            Box(
                Modifier.padding(top = 6.dp, end = 8.dp).size(10.dp).clip(CircleShape)
                    .background(if (model.phase == AppModel.Phase.Connected) Color.Green else Orange),
            )
            Column(Modifier.weight(1f)) {
                Text(statusLine(model), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (model.phase != AppModel.Phase.Connected) model.errorMessage?.let { Hint(it) }
            }
            Column(horizontalAlignment = Alignment.End, modifier = Modifier.padding(start = 8.dp)) {
                if (model.channel > 0) Text("Phone ${model.slot} · Ch ${model.channel}", fontWeight = FontWeight.SemiBold)
                model.latencyMs?.let { Hint("≈ ${it.toInt()} ms delay") }
            }
        }

        if (model.interrupted) {
            Banner("Another app or a call has taken the microphone. Audio will resume when it ends.", Orange.copy(alpha = 0.3f))
        } else if (model.showFocusTip) {
            Banner("Turn on Do Not Disturb so calls and notifications can't interrupt you on air.", Color(0xFF2B2F66), onDismiss = { model.showFocusTip = false })
        }

        val look = buttonLook(model)
        Column(
            Modifier.weight(1f).fillMaxWidth().clip(RoundedCornerShape(36.dp)).background(look.color)
                .clickable(onClick = model::toggleMute).padding(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Text(look.title, color = Color.White, fontSize = 64.sp, fontWeight = FontWeight.Black)
            Spacer(Modifier.height(12.dp))
            Text(look.subtitle, color = Color.White, fontSize = 20.sp, textAlign = TextAlign.Center)
        }

        LevelMeter(model.levelDb, dimmed = !model.isLive)

        Row(verticalAlignment = Alignment.CenterVertically) {
            MicPicker(model, Modifier.weight(1f))
            TextButton(onClick = { confirmDisconnect = true }) { Text("Disconnect", color = MutedRed) }
        }
    }
    if (confirmDisconnect) {
        AlertDialog(
            onDismissRequest = { confirmDisconnect = false },
            title = { Text("Stop sending audio?") },
            confirmButton = { TextButton(onClick = { confirmDisconnect = false; model.disconnect() }) { Text("Disconnect", color = MutedRed) } },
            dismissButton = { TextButton(onClick = { confirmDisconnect = false }) { Text("Cancel") } },
        )
    }
}

private fun statusLine(model: AppModel) = when (model.phase) {
    AppModel.Phase.Connected -> "Connected · ${model.desktopName}"
    AppModel.Phase.Connecting -> "Connecting to ${model.target?.label ?: "desktop"}…"
    AppModel.Phase.Reconnecting -> "Reconnecting to ${model.target?.label ?: "desktop"}…"
    AppModel.Phase.Idle -> ""
}

private data class ButtonLook(val title: String, val subtitle: String, val color: Color)

private fun buttonLook(model: AppModel): ButtonLook = when {
    model.phase != AppModel.Phase.Connected ->
        ButtonLook("OFFLINE", if (model.talentMuted) "Not connected · you are muted" else "Not connected to the desktop", Color.Gray)
    model.talentMuted -> ButtonLook("MUTED", "Tap to go live", MutedRed)
    model.operatorMuted -> ButtonLook("MUTED", "Muted by the board operator", MutedRed)
    model.interrupted -> ButtonLook("NO MIC", "Microphone in use by another app", Orange)
    model.micStalled -> ButtonLook("NO MIC", "The microphone isn't sending audio. Trying to restart it…", Orange)
    else -> ButtonLook("LIVE", "Tap to mute", LiveGreen)
}

@Composable
private fun MicPicker(model: AppModel, modifier: Modifier) {
    var open by remember { mutableStateOf(false) }
    Box(modifier) {
        TextButton(onClick = { open = true }) { Text("Mic: ${model.currentMic?.name ?: "Microphone"}", maxLines = 1, overflow = TextOverflow.Ellipsis) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            model.mics.forEach { mic ->
                DropdownMenuItem(
                    text = { Text(if (mic.id == model.currentMic?.id) "✓ ${mic.name}" else mic.name) },
                    onClick = { open = false; model.selectMic(mic) },
                )
            }
        }
    }
}

/** Horizontal level meter, −60…0 dBFS. */
@Composable
private fun LevelMeter(db: Float, dimmed: Boolean) {
    val fraction = ((db + 60) / 60).coerceIn(0f, 1f)
    Box(Modifier.fillMaxWidth().height(14.dp).clip(CircleShape).background(Color.White.copy(alpha = 0.12f))) {
        Box(
            Modifier.fillMaxHeight().fillMaxWidth(fraction).clip(CircleShape)
                .background(Brush.horizontalGradient(listOf(Color.Green, Color.Green, Color.Yellow, Color.Red)), alpha = if (dimmed) 0.35f else 1f),
        )
    }
}

@Composable
private fun SettingsScreen(model: AppModel, onDone: () -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Settings", fontSize = 24.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            TextButton(onClick = onDone) { Text("Done") }
        }
        SwitchRow("Noise suppression", model.noiseSuppression, enabled = true, onChange = model::updateNoiseSuppression)
        SwitchRow("Auto gain", model.autoGain, enabled = true, onChange = model::updateAutoGain)
        Hint("Leave these off unless there's a noise problem: the mixer's Fat Channel normally does this job. They can change the sound, and not every phone supports them.")
    }
}

@Composable
private fun SwitchRow(label: String, checked: Boolean, enabled: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, modifier = Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange, enabled = enabled)
    }
}

@Composable
private fun Banner(text: String, color: Color, onDismiss: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(color).padding(start = 12.dp, top = 8.dp, bottom = 8.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, modifier = Modifier.weight(1f), fontSize = 14.sp)
        if (onDismiss != null) TextButton(onClick = onDismiss) { Text("✕") }
    }
}

@Composable
private fun Hint(text: String) {
    Text(text, fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
private fun ErrorText(text: String?) {
    if (text != null) Text(text, color = Color(0xFFFF6B6B), fontSize = 14.sp)
}
