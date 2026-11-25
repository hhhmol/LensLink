package io.github.lenslink.stream

import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/** Wire protocol v1, see docs/protocol.md. Mirrors pc/crates/lenslink-protocol. */
object Protocol {
    val MAGIC = byteArrayOf('L'.code.toByte(), 'N'.code.toByte(), 'S'.code.toByte(), 'L'.code.toByte())
    const val VERSION = 1
    const val SOCKET_NAME = "lenslink"
    const val MAX_PAYLOAD = 16 * 1024 * 1024

    const val KIND_CONTROL = 0
    const val KIND_VIDEO = 1
    const val KIND_AUDIO = 2

    const val FLAG_CONFIG = 1
    const val FLAG_KEYFRAME = 2
    const val FLAG_END_OF_STREAM = 4
}

class Packet(val kind: Int, val flags: Int, val ptsUs: Long, val payload: ByteArray)

/** Thread-safe: the camera thread writes video while the session thread writes control replies. */
class PacketWriter(stream: OutputStream) {
    private val out = DataOutputStream(BufferedOutputStream(stream, 1 shl 16))

    @Synchronized
    fun handshake() {
        out.write(Protocol.MAGIC)
        out.writeShort(Protocol.VERSION)
        out.writeShort(0)
        out.flush()
    }

    @Synchronized
    fun packet(kind: Int, flags: Int, ptsUs: Long, payload: ByteArray, offset: Int = 0, length: Int = payload.size) {
        require(length <= Protocol.MAX_PAYLOAD) { "payload of $length bytes is too large" }
        out.writeByte(kind)
        out.writeByte(flags)
        out.writeShort(0)
        out.writeInt(length)
        out.writeLong(ptsUs)
        out.write(payload, offset, length)
        out.flush()
    }

    fun control(message: JSONObject) =
        packet(Protocol.KIND_CONTROL, 0, 0, message.toString().toByteArray(Charsets.UTF_8))
}

class PacketReader(stream: InputStream) {
    private val input = DataInputStream(BufferedInputStream(stream, 1 shl 14))

    fun handshake() {
        val magic = ByteArray(4)
        input.readFully(magic)
        if (!magic.contentEquals(Protocol.MAGIC)) throw IOException("peer is not a LensLink host")
        val version = input.readUnsignedShort()
        input.readUnsignedShort() // reserved
        if (version != Protocol.VERSION) throw IOException("host speaks protocol v$version, phone speaks v${Protocol.VERSION}")
    }

    /** Next packet, or null when the host closed the connection between packets. */
    fun next(): Packet? {
        val kind = input.read()
        if (kind < 0) return null
        try {
            val flags = input.readUnsignedByte()
            input.readUnsignedShort() // reserved
            val length = input.readInt()
            if (length < 0 || length > Protocol.MAX_PAYLOAD) throw IOException("bad payload length $length")
            val pts = input.readLong()
            val payload = ByteArray(length)
            input.readFully(payload)
            return Packet(kind, flags, pts, payload)
        } catch (e: EOFException) {
            throw IOException("connection closed in the middle of a packet", e)
        }
    }
}
