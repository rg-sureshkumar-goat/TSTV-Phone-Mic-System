package org.tstvnews.phonemic

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class WireTest {
    private fun bytes(vararg values: Int) = values.map { it.toByte() }.toByteArray()

    /** The same packet the desktop's protocol test builds, byte for byte. */
    @Test fun audioPacketMatchesTheProtocol() {
        val packet = Wire.audioPacket(0xDEADBEEF.toInt(), 42, muted = true, primary = bytes(1, 2, 3), redundant = bytes(9, 8))
        val expected = bytes(
            0x54, 0x56, 1, 1, // "TV", version, audio
            0xDE, 0xAD, 0xBE, 0xEF, // token
            0, 0, 0, 42, // sequence
            1, 1, // muted, Opus
            0, 3, 1, 2, 3, // primary
            0, 2, 9, 8, // redundant
        )
        assertArrayEquals(expected, packet)
    }

    @Test fun pingGetsMatchingPong() {
        val ping = bytes(0x54, 0x56, 1, 2, 0, 0, 0, 7, 1, 2, 3, 4, 5, 6, 7, 8, 99)
        val pong = Wire.pongFor(ping, 16)!!
        assertEquals(16, pong.size)
        assertEquals(3, pong[3].toInt())
        assertEquals(8, pong[15].toInt())
    }

    @Test fun otherDatagramsGetNoPong() {
        assertNull(Wire.pongFor(ByteArray(16), 16))
        assertNull(Wire.pongFor(bytes(0x54, 0x56, 1, 2), 4))
    }

    @Test fun joinLinkFromTheDesktopParses() {
        // Exact output of the desktop's join_link test.
        val link = JoinLink.parse("tstvmic://join?host=10.0.0.5&host=2607%3Afb90%3A%3A1&name=TSTV%20%28RG%E2%80%99s%20Mac%29&code=4821")!!
        assertEquals(listOf("10.0.0.5", "2607:fb90::1"), link.hosts)
        assertEquals("TSTV (RG’s Mac)", link.name)
        assertEquals("4821", link.code)
    }

    @Test fun otherLinksAreRefused() {
        assertNull(JoinLink.parse("https://example.com/join?host=1.2.3.4"))
        assertNull(JoinLink.parse("tstvmic://join?name=NoAddress"))
    }

    @Test fun desktopMessagesParse() {
        val welcome = DesktopMessage.parse("""{"type":"welcome","protocol":1,"slot":2,"channel":18,"token":2882400018,"udpPort":47801,"bitrate":128000,"desktopName":"Mac"}""") as DesktopMessage.Welcome
        assertEquals(2, welcome.slot)
        assertEquals(18, welcome.channel)
        assertEquals(2882400018L.toInt(), welcome.token)
        val reject = DesktopMessage.parse("""{"type":"reject","reason":"code","message":"Needs a code"}""") as DesktopMessage.Reject
        assertEquals("code", reject.reason)
        assertNull(DesktopMessage.parse("""{"type":"somethingNew"}"""))
        assertNull(DesktopMessage.parse("not json"))
    }
}
