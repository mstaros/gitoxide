namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>
    /// Experimental P0a entry point for pulling an already materialized Git object
    /// into a caller-owned reusable buffer.
    /// </summary>
    internal PrototypeByteReaderStream OpenObjectByteStreamForPrototype(GixObjectId objectId) =>
        Invoke("OpenObjectByteStreamForPrototype", repo =>
        {
            using var nativeId = objectId.Value.Utf8();
            return new PrototypeByteReaderStream(ByteReader.FromObject(repo, nativeId));
        });

    /// <summary>
    /// Experimental P0a entry point for gix's pipe-backed transient worktree stream.
    /// </summary>
    internal PrototypeByteReaderStream OpenWorktreeByteStreamForPrototype(GixObjectId treeId) =>
        Invoke("OpenWorktreeByteStreamForPrototype", repo =>
        {
            using var nativeId = treeId.Value.Utf8();
            return new PrototypeByteReaderStream(ByteReader.FromWorktree(repo, nativeId));
        });

    /// <summary>
    /// A deliberately local prototype, not a general Interoptopus streaming abstraction.
    /// The native reader never retains the managed destination pointer.
    /// </summary>
    internal sealed class PrototypeByteReaderStream : Stream
    {
        private readonly object _sync = new();
        private readonly SliceMutByte _destination =
            SliceMutByte.CreateUnownedForByteStreamingPrototype();
        private ByteReader? _reader;

        internal PrototypeByteReaderStream(ByteReader reader) =>
            _reader = reader;

        public override bool CanRead
        {
            get
            {
                lock (_sync)
                    return _reader is not null;
            }
        }

        public override bool CanSeek => false;

        public override bool CanWrite => false;

        public override long Length => WithReader(
            "ByteReader.Length",
            static reader => reader.HasKnownLength()
                ? checked((long)reader.KnownLength())
                : throw new NotSupportedException("This streaming source has no cheap known length."));

        public override long Position
        {
            get => checked((long)BytesDelivered);
            set => throw new NotSupportedException();
        }

        internal bool EofObserved =>
            WithReader("ByteReader.EofObserved", static reader => reader.EofObserved());

        internal bool HasKnownLength =>
            WithReader("ByteReader.HasKnownLength", static reader => reader.HasKnownLength());

        internal ulong KnownLength =>
            WithReader("ByteReader.KnownLength", static reader => reader.KnownLength());

        internal ulong RetainedNativeBytes =>
            WithReader("ByteReader.RetainedNativeBytes", static reader => reader.RetainedNativeBytes());

        internal ulong BytesDelivered =>
            WithReader("ByteReader.BytesDelivered", static reader => reader.BytesDelivered());

        internal ulong FfiReadCalls =>
            WithReader("ByteReader.ReadCalls", static reader => reader.ReadCalls());

        public override void Flush()
        {
            ObjectDisposedException.ThrowIf(!CanRead, this);
        }

        public override int Read(byte[] buffer, int offset, int count)
        {
            ArgumentNullException.ThrowIfNull(buffer);
            if ((uint)offset > (uint)buffer.Length)
                throw new ArgumentOutOfRangeException(nameof(offset));
            if ((uint)count > (uint)(buffer.Length - offset))
                throw new ArgumentOutOfRangeException(nameof(count));

            return Read(buffer.AsSpan(offset, count));
        }

        public override unsafe int Read(Span<byte> buffer)
        {
            lock (_sync)
            {
                var reader = _reader ??
                    throw new ObjectDisposedException(nameof(PrototypeByteReaderStream));

                try
                {
                    // A non-null sentinel keeps the zero-length slice valid without
                    // allocating. The pointer is valid only for this synchronous call.
                    byte emptySentinel = 0;
                    fixed (byte* pinned = buffer)
                    {
                        var destination = buffer.IsEmpty ? &emptySentinel : pinned;
                        _destination.PointAtUnownedForByteStreamingPrototype(
                            (IntPtr)destination,
                            (ulong)buffer.Length);
                        try
                        {
                            return checked((int)reader.Read(_destination));
                        }
                        finally
                        {
                            _destination.InvalidateUnownedForByteStreamingPrototype();
                        }
                    }
                }
                catch (EnumException<GixError> exception)
                {
                    throw Translate("ByteReader.Read", exception);
                }
                catch (InteropException exception)
                {
                    throw Unexpected("ByteReader.Read", exception);
                }
            }
        }

        public override ValueTask<int> ReadAsync(
            Memory<byte> buffer,
            CancellationToken cancellationToken = default)
        {
            cancellationToken.ThrowIfCancellationRequested();
            return ValueTask.FromResult(Read(buffer.Span));
        }

        public override long Seek(long offset, SeekOrigin origin) =>
            throw new NotSupportedException();

        public override void SetLength(long value) =>
            throw new NotSupportedException();

        public override void Write(byte[] buffer, int offset, int count) =>
            throw new NotSupportedException();

        protected override void Dispose(bool disposing)
        {
            if (disposing)
            {
                lock (_sync)
                {
                    var reader = _reader;
                    _reader = null;
                    _destination.Dispose();
                    reader?.Dispose();
                }
            }

            base.Dispose(disposing);
        }

        private T WithReader<T>(string operation, Func<ByteReader, T> action)
        {
            lock (_sync)
            {
                var reader = _reader ??
                    throw new ObjectDisposedException(nameof(PrototypeByteReaderStream));
                return InvokeStatic(operation, () => action(reader));
            }
        }
    }
}

// Interoptopus emits SliceMutByte as partial. Reusing one unowned view avoids a
// managed wrapper allocation per pull while the Stream lock prevents overlap.
public partial class SliceMutByte
{
    internal static SliceMutByte CreateUnownedForByteStreamingPrototype() => new();

    internal void PointAtUnownedForByteStreamingPrototype(IntPtr data, ulong length)
    {
        if (_handle.IsAllocated)
            throw new InvalidOperationException("The prototype view must not own a managed pin.");

        _data = data;
        _len = length;
    }

    internal void InvalidateUnownedForByteStreamingPrototype()
    {
        _data = IntPtr.Zero;
        _len = 0;
    }
}
