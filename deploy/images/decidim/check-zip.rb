# frozen_string_literal: true

# Proves on the built image what the widened rubyzip range in the Dockerfile rests on: every gem
# that touches a zip still works on rubyzip 3, and a hostile entry stays out of its destination
# (T-6448). `bin/rails runner db/joinedcontext_check_zip.rb`, no database: exits 1 on any failure.
require "tmpdir"
require "zip"

def check(name)
  raise "#{name}: failed" unless yield

  puts "ok #{name}"
end

check("rubyzip is past the CVE-2026-85396 fix") { Gem::Version.new(Zip::VERSION) >= Gem::Version.new("3.4.0") }

check("decidim-core FileZipper writes an archive that reads back") do
  buffer = Decidim::FileZipper.new("data.txt", "payload").zip
  Zip::File.open_buffer(buffer) { |zip| zip.read("data.txt") == "payload" }
end

check("rubyXL writes and parses a workbook") do
  book = RubyXL::Workbook.new
  book[0].add_cell(0, 0, "value")
  RubyXL::Parser.parse_buffer(book.stream.string)[0][0][0].value == "value"
end

Dir.mktmpdir do |dir|
  docx = File.join(dir, "text.docx")
  Zip::OutputStream.open(docx) do |zip|
    zip.put_next_entry("[Content_Types].xml")
    zip.write("<Types/>")
    zip.put_next_entry("word/styles.xml")
    zip.write('<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>')
    zip.put_next_entry("word/document.xml")
    zip.write(<<~XML)
      <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
        <w:body><w:p><w:r><w:t>Participacia</w:t></w:r></w:p></w:body>
      </w:document>
    XML
  end
  check("doc2text converts a docx") do
    out = File.join(dir, "out.md")
    Doc2Text::Docx::Document.parse_and_save(docx, out)
    File.read(out).include?("Participacia")
  end

  hostile = File.join(dir, "hostile.zip")
  Zip::OutputStream.open(hostile) do |zip|
    zip.put_next_entry("../upload_backup/owned.sh")
    zip.write("x")
  end
  base = File.join(dir, "upload")
  Dir.mkdir(base)
  check("a sibling-prefix entry stays out of the extraction directory") do
    Zip.validate_entry_sizes = true
    Zip::File.open(hostile) do |zip|
      zip.each do |entry|
        zip.extract(entry, nil, destination_directory: base)
      rescue Zip::Error, ArgumentError
        nil
      end
    end
    !File.exist?(File.join(dir, "upload_backup/owned.sh"))
  end
end
