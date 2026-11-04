package org.tstvnews.phonemic

import android.app.Application

/** Holds the app state, so streaming outlives the screen. */
class TstvApp : Application() {
    val model by lazy { AppModel(this) }
}
