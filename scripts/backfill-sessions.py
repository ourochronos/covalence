#!/usr/bin/env python3
"""
Backfill OpenClaw session transcripts into Covalence.

This script reads JSONL session files, extracts user/assistant messages,
chunks them appropriately, and ingests them into Covalence as conversation sources.

Usage:
    python scripts/backfill-sessions.py [--dry-run] [--test] [--session-dir PATH]
"""

import argparse
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import List, Dict, Any, Optional
import requests
from datetime import datetime


@dataclass
class Message:
    """Extracted message from session transcript."""
    role: str
    content: str
    timestamp: str


class SessionBackfiller:
    """Handles backfilling session transcripts into Covalence."""
    
    def __init__(
        self,
        covalence_url: str = "http://localhost:8430",
        min_file_size: int = 100_000,
        chunk_size: int = 50,
        chunk_char_limit: int = 100_000,
        dry_run: bool = False
    ):
        self.covalence_url = covalence_url
        self.min_file_size = min_file_size
        self.chunk_size = chunk_size
        self.chunk_char_limit = chunk_char_limit
        self.dry_run = dry_run
        
        self.stats = {
            'files_processed': 0,
            'files_skipped': 0,
            'files_failed': 0,
            'chunks_ingested': 0,
            'messages_extracted': 0,
            'errors': []
        }
    
    def extract_content_text(self, content: Any) -> str:
        """Extract text from content (handles string and array-of-blocks formats)."""
        if isinstance(content, str):
            return content
        
        if isinstance(content, list):
            parts = []
            for block in content:
                if isinstance(block, dict):
                    # Extract text from text blocks, skip tool calls
                    if block.get('type') == 'text':
                        parts.append(block.get('text', ''))
                    # Note: we skip toolCall, toolResult, thinking blocks
                elif isinstance(block, str):
                    parts.append(block)
            return '\n'.join(parts)
        
        return str(content)
    
    def extract_messages(self, session_file: Path) -> tuple[str, List[Message]]:
        """Extract user/assistant messages from a session JSONL file."""
        messages = []
        session_id = session_file.stem  # filename without extension
        
        try:
            with open(session_file, 'r', encoding='utf-8') as f:
                for line_num, line in enumerate(f, 1):
                    line = line.strip()
                    if not line:
                        continue
                    
                    try:
                        entry = json.loads(line)
                    except json.JSONDecodeError as e:
                        print(f"  ⚠️  JSON decode error at line {line_num}: {e}", file=sys.stderr)
                        continue
                    
                    # We only care about type=message with role=user or role=assistant
                    if entry.get('type') != 'message':
                        continue
                    
                    message_data = entry.get('message', {})
                    role = message_data.get('role')
                    
                    if role not in ('user', 'assistant'):
                        continue
                    
                    content = message_data.get('content')
                    if not content:
                        continue
                    
                    text = self.extract_content_text(content)
                    if not text.strip():
                        continue
                    
                    timestamp = entry.get('timestamp', '')
                    
                    messages.append(Message(
                        role=role,
                        content=text,
                        timestamp=timestamp
                    ))
        
        except Exception as e:
            raise Exception(f"Failed to read {session_file}: {e}")
        
        return session_id, messages
    
    def chunk_messages(self, messages: List[Message]) -> List[List[Message]]:
        """Split messages into chunks respecting size limits."""
        if not messages:
            return []
        
        chunks = []
        current_chunk = []
        current_chars = 0
        
        for msg in messages:
            msg_chars = len(msg.content)
            
            # Check if adding this message would exceed limits
            if (len(current_chunk) >= self.chunk_size or 
                current_chars + msg_chars > self.chunk_char_limit):
                
                if current_chunk:
                    chunks.append(current_chunk)
                    current_chunk = []
                    current_chars = 0
            
            current_chunk.append(msg)
            current_chars += msg_chars
        
        # Add the last chunk
        if current_chunk:
            chunks.append(current_chunk)
        
        return chunks
    
    def format_chunk_content(self, messages: List[Message]) -> str:
        """Format messages into a readable conversation transcript."""
        lines = []
        for msg in messages:
            timestamp = msg.timestamp
            role = msg.role.upper()
            lines.append(f"[{timestamp}] {role}:")
            lines.append(msg.content)
            lines.append("")  # blank line between messages
        
        return '\n'.join(lines)
    
    def ingest_chunk(
        self,
        session_id: str,
        chunk_index: int,
        chunk: List[Message]
    ) -> bool:
        """Ingest a single chunk into Covalence."""
        
        content = self.format_chunk_content(chunk)
        
        first_timestamp = chunk[0].timestamp if chunk else ""
        last_timestamp = chunk[-1].timestamp if chunk else ""
        
        metadata = {
            'session_id': session_id,
            'chunk_index': chunk_index,
            'message_count': len(chunk),
            'first_timestamp': first_timestamp,
            'last_timestamp': last_timestamp
        }
        
        payload = {
            'content': content,
            'source_type': 'conversation',
            'title': f'Session transcript: {session_id} chunk {chunk_index}',
            'idempotency_key': f'session:{session_id}:chunk:{chunk_index}',
            'metadata': metadata
        }
        
        if self.dry_run:
            print(f"  [DRY RUN] Would ingest chunk {chunk_index}: "
                  f"{len(chunk)} messages, {len(content)} chars")
            return True
        
        try:
            response = requests.post(
                f"{self.covalence_url}/sources",
                json=payload,
                timeout=30
            )
            
            if response.status_code in (200, 201, 409):
                # 200/201 = success, 409 = already exists (idempotent)
                if response.status_code == 409:
                    print(f"  ✓ Chunk {chunk_index} already exists (idempotent)")
                else:
                    print(f"  ✓ Ingested chunk {chunk_index}: "
                          f"{len(chunk)} messages, {len(content)} chars")
                return True
            else:
                error_msg = f"HTTP {response.status_code}: {response.text[:200]}"
                print(f"  ✗ Failed to ingest chunk {chunk_index}: {error_msg}", 
                      file=sys.stderr)
                self.stats['errors'].append({
                    'session_id': session_id,
                    'chunk_index': chunk_index,
                    'error': error_msg
                })
                return False
        
        except Exception as e:
            error_msg = str(e)
            print(f"  ✗ Exception ingesting chunk {chunk_index}: {error_msg}", 
                  file=sys.stderr)
            self.stats['errors'].append({
                'session_id': session_id,
                'chunk_index': chunk_index,
                'error': error_msg
            })
            return False
    
    def process_session(self, session_file: Path) -> bool:
        """Process a single session file."""
        try:
            # Extract messages
            session_id, messages = self.extract_messages(session_file)
            
            if not messages:
                print(f"  ⚠️  No messages found, skipping")
                self.stats['files_skipped'] += 1
                return False
            
            print(f"  Extracted {len(messages)} messages")
            self.stats['messages_extracted'] += len(messages)
            
            # Chunk messages
            chunks = self.chunk_messages(messages)
            print(f"  Split into {len(chunks)} chunks")
            
            # Ingest each chunk
            success_count = 0
            for i, chunk in enumerate(chunks):
                if self.ingest_chunk(session_id, i, chunk):
                    success_count += 1
                    if not self.dry_run:
                        self.stats['chunks_ingested'] += 1
            
            if success_count == len(chunks):
                self.stats['files_processed'] += 1
                return True
            else:
                print(f"  ⚠️  Only {success_count}/{len(chunks)} chunks succeeded")
                self.stats['files_failed'] += 1
                return False
        
        except Exception as e:
            print(f"  ✗ Failed to process: {e}", file=sys.stderr)
            self.stats['files_failed'] += 1
            self.stats['errors'].append({
                'session_file': str(session_file),
                'error': str(e)
            })
            return False
    
    def find_session_files(self, session_dir: Path) -> List[Path]:
        """Find session JSONL files meeting size requirements."""
        files = []
        
        for path in session_dir.glob('*.jsonl'):
            # Skip deleted/reset sessions
            if '.deleted.' in path.name or '.reset.' in path.name:
                continue
            
            size = path.stat().st_size
            if size >= self.min_file_size:
                files.append(path)
        
        return sorted(files)
    
    def run(self, session_dir: Path, test_mode: bool = False):
        """Run the backfill process."""
        print(f"🔍 Scanning {session_dir} for session files...")
        
        files = self.find_session_files(session_dir)
        total_files = len(files)
        
        print(f"📊 Found {total_files} session files ≥{self.min_file_size} bytes")
        
        if test_mode:
            print("🧪 TEST MODE: Processing only first 3 files")
            files = files[:3]
        
        if not files:
            print("No files to process")
            return
        
        print()
        
        for i, session_file in enumerate(files, 1):
            size_kb = session_file.stat().st_size / 1024
            print(f"[{i}/{len(files)}] Processing {session_file.name} ({size_kb:.1f} KB)")
            self.process_session(session_file)
            print()
        
        self.print_summary(total_files)
    
    def print_summary(self, total_available: int):
        """Print final statistics."""
        print("=" * 60)
        print("BACKFILL SUMMARY")
        print("=" * 60)
        print(f"Total session files found:    {total_available}")
        print(f"Files processed successfully: {self.stats['files_processed']}")
        print(f"Files skipped (no messages):  {self.stats['files_skipped']}")
        print(f"Files failed:                 {self.stats['files_failed']}")
        print(f"Total messages extracted:     {self.stats['messages_extracted']}")
        print(f"Total chunks ingested:        {self.stats['chunks_ingested']}")
        
        if self.stats['errors']:
            print(f"\n❌ Errors encountered: {len(self.stats['errors'])}")
            for i, error in enumerate(self.stats['errors'][:10], 1):
                print(f"  {i}. {error}")
            if len(self.stats['errors']) > 10:
                print(f"  ... and {len(self.stats['errors']) - 10} more")
        
        if self.dry_run:
            print("\n[DRY RUN MODE - No data was actually ingested]")
        
        print("=" * 60)


def main():
    parser = argparse.ArgumentParser(
        description='Backfill OpenClaw session transcripts into Covalence'
    )
    parser.add_argument(
        '--session-dir',
        type=Path,
        default=Path.home() / '.openclaw/agents/main/sessions',
        help='Path to session JSONL files'
    )
    parser.add_argument(
        '--covalence-url',
        default='http://localhost:8430',
        help='Covalence API base URL'
    )
    parser.add_argument(
        '--min-size',
        type=int,
        default=100_000,
        help='Minimum file size in bytes (default: 100KB)'
    )
    parser.add_argument(
        '--chunk-messages',
        type=int,
        default=50,
        help='Max messages per chunk'
    )
    parser.add_argument(
        '--chunk-chars',
        type=int,
        default=100_000,
        help='Max characters per chunk'
    )
    parser.add_argument(
        '--test',
        action='store_true',
        help='Test mode: process only first 3 files'
    )
    parser.add_argument(
        '--dry-run',
        action='store_true',
        help='Dry run: show what would be done without ingesting'
    )
    
    args = parser.parse_args()
    
    if not args.session_dir.exists():
        print(f"Error: Session directory not found: {args.session_dir}", 
              file=sys.stderr)
        sys.exit(1)
    
    backfiller = SessionBackfiller(
        covalence_url=args.covalence_url,
        min_file_size=args.min_size,
        chunk_size=args.chunk_messages,
        chunk_char_limit=args.chunk_chars,
        dry_run=args.dry_run
    )
    
    try:
        backfiller.run(args.session_dir, test_mode=args.test)
    except KeyboardInterrupt:
        print("\n\n⚠️  Interrupted by user")
        backfiller.print_summary(0)
        sys.exit(130)
    except Exception as e:
        print(f"\n\n❌ Fatal error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
