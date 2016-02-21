use std::io::Write;
use std::io::Read;
use std::io::Cursor;
use std::io::SeekFrom;
use std::io::Seek;
use std::marker::PhantomData;
use core::DocId;
use core::error;
use byteorder;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use core::serialize::*;

struct LayerBuilder<T: BinarySerializable> {
    period: usize,
    buffer: Vec<u8>,
    remaining: usize,
    len: usize,
    _phantom_: PhantomData<T>,
}

impl<T: BinarySerializable> LayerBuilder<T> {

    fn written_size(&self,) -> usize {
        self.buffer.len()
    }

    fn write(&self, output: &mut Write) -> Result<(), byteorder::Error> {
        try!(output.write_all(&self.buffer));
        Ok(())
    }
    
    fn with_period(period: usize) -> LayerBuilder<T> {
        LayerBuilder {
            period: period,
            buffer: Vec::new(),
            remaining: period,
            len: 0,
            _phantom_: PhantomData,
        }
    }

    fn insert(&mut self, doc_id: DocId, value: &T) -> Option<(DocId, u32)> {
        self.remaining -= 1;
        self.len += 1;
        let offset = self.written_size() as u32; // TODO not sure if we want after or here
        let res;
        if self.remaining == 0 {
            self.remaining = self.period;
            res = Some((doc_id, offset));
        }
        else {
            res = None;
        }
        self.buffer.write_u32::<BigEndian>(doc_id);
        value.serialize(&mut self.buffer);
        res
    }
}

pub struct SkipListBuilder<T: BinarySerializable> {
    period: usize,
    data_layer: LayerBuilder<T>,
    skip_layers: Vec<LayerBuilder<u32>>,
}


impl<T: BinarySerializable> SkipListBuilder<T> {

    pub fn new(period: usize) -> SkipListBuilder<T> {
        SkipListBuilder {
            period: period,
            data_layer: LayerBuilder::with_period(period),
            skip_layers: Vec::new(),
        }
    }

    fn get_skip_layer<'a>(&'a mut self, layer_id: usize) -> &mut LayerBuilder<u32> {
        if layer_id == self.skip_layers.len() {
            let layer_builder = LayerBuilder::with_period(self.period);
            self.skip_layers.push(layer_builder);
        }
        &mut self.skip_layers[layer_id]
    }

    pub fn insert(&mut self, doc_id: DocId, dest: &T) {
        let mut layer_id = 0;
        let mut skip_pointer = self.data_layer.insert(doc_id, dest);
        loop {
            skip_pointer = match skip_pointer {
                Some((skip_doc_id, skip_offset)) =>
                    self
                        .get_skip_layer(layer_id)
                        .insert(skip_doc_id, &skip_offset),
                None => { return; }
            };
            layer_id += 1;
        }
    }

    pub fn write<W: Write>(self, output: &mut Write) -> error::Result<()> {
        let mut size: u32 = 0;
        let mut layer_sizes: Vec<u32> = Vec::new();
        size += self.data_layer.buffer.len() as u32;
        layer_sizes.push(size);
        for layer in self.skip_layers.iter() {
            size += layer.buffer.len() as u32;
            layer_sizes.push(size);
        }
        layer_sizes.serialize(output);
        match self.data_layer.write(output) {
            Ok(())=> {},
            Err(someerr)=> { return Err(error::Error::WriteError(format!("Could not write skiplist {:?}", someerr) )) }
        }
        for layer in self.skip_layers.iter() {
            match layer.write(output) {
                Ok(())=> {},
                Err(someerr)=> { return Err(error::Error::WriteError(format!("Could not write skiplist {:?}", someerr) )) }
            }
        }
        Ok(())
    }
}


struct Layer<'a, T> {
    cursor: Cursor<&'a [u8]>,
    next_id: DocId,
    _phantom_: PhantomData<T>,
}


impl<'a, T: BinarySerializable> Iterator for Layer<'a, T> {

    type Item = (DocId, T);

    fn next(&mut self,)-> Option<(DocId, T)> {
        if self.next_id == u32::max_value() {
            None
        }
        else {
            let cur_val = T::deserialize(&mut self.cursor).unwrap();
            let cur_id = self.next_id;
            self.next_id =
                match u32::deserialize(&mut self.cursor) {
                    Ok(val) => val,
                    Err(_) => u32::max_value()
                };
            Some((cur_id, cur_val))
        }
    }
}


static EMPTY: [u8; 0] = [];

impl<'a, T: BinarySerializable> Layer<'a, T> {

    fn read(mut cursor: Cursor<&'a [u8]>) -> Layer<'a, T> {
        // TODO error handling?
        let next_id = match cursor.read_u32::<BigEndian>() {
            Ok(val) => val,
            Err(_) => u32::max_value(),
        };
        Layer {
            cursor: cursor,
            next_id: next_id,
            _phantom_: PhantomData,
        }
    }

    fn empty() -> Layer<'a, T> {
        Layer {
            cursor: Cursor::new(&EMPTY),
            next_id: DocId::max_value(),
            _phantom_: PhantomData,
        }
    }


    fn seek_offset(&mut self, offset: usize) {
        self.cursor.seek(SeekFrom::Start(offset as u64));
        self.next_id = match self.cursor.read_u32::<BigEndian>() {
            Ok(val) => val,
            Err(_) => u32::max_value(),
        };
    }

    // Returns the last element (key, val)
    // such that (key < doc_id)
    //
    // If there is no such element anymore,
    // returns None.
    fn seek(&mut self, doc_id: DocId) -> Option<(DocId, T)> {
        let mut val = None;
        while self.next_id < doc_id {
            match self.next() {
                None => { break; },
                v => { val = v; }
            }
        }
        val
    }
}

pub struct SkipList<'a, T: BinarySerializable> {
    data_layer: Layer<'a, T>,
    skip_layers: Vec<Layer<'a, u32>>,
}

impl<'a, T: BinarySerializable> Iterator for SkipList<'a, T> {

    type Item = (DocId, T);

    fn next(&mut self,)-> Option<(DocId, T)> {
        self.data_layer.next()
    }
}

impl<'a, T: BinarySerializable> SkipList<'a, T> {

    pub fn seek(&mut self, doc_id: DocId) -> Option<(DocId, T)> {
        let mut next_layer_skip: Option<(DocId, u32)> = None;
        for skip_layer_id in 0..self.skip_layers.len() {
            let mut skip_layer: &mut Layer<'a, u32> = &mut self.skip_layers[skip_layer_id];
            match next_layer_skip {
                 Some((_, offset)) => { skip_layer.seek_offset(offset as usize); },
                 None => {}
             };
             next_layer_skip = skip_layer.seek(doc_id);
         }
         match next_layer_skip {
             Some((_, offset)) => { self.data_layer.seek_offset(offset as usize); },
             None => {}
         };
         self.data_layer.seek(doc_id)
    }

    pub fn read(data: &'a [u8]) -> SkipList<'a, T> {
        let mut cursor = Cursor::new(data);
        let offsets: Vec<u32> = Vec::deserialize(&mut cursor).unwrap();
        let num_layers = offsets.len();
        let start_position = cursor.position() as usize;
        let layers_data: &[u8] = &data[start_position..data.len()];
        let data_layer: Layer<'a, T> =
            if num_layers == 0 { Layer::empty() }
            else {
                let first_layer_data: &[u8] = &layers_data[..offsets[0] as usize];
                let first_layer_cursor = Cursor::new(first_layer_data);
                Layer::read(first_layer_cursor)
            };
        let mut skip_layers =
            if num_layers > 0 {
                offsets.iter()
                    .zip(&offsets[1..])
                    .map(|(start, stop)| {
                        let layer_data: &[u8] = &layers_data[*start as usize..*stop as usize];
                        let cursor = Cursor::new(layer_data);
                        Layer::read(cursor)
                    })
                    .collect()
            }
            else {
                Vec::new()
            };
        skip_layers.reverse();
        SkipList {
            skip_layers: skip_layers,
            data_layer: data_layer,
        }
    }
}




#[test]
fn test_skip_list_builder() {
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<u32> = SkipListBuilder::new(10);
        skip_list_builder.insert(2, &3);
        skip_list_builder.write::<Vec<u8>>(&mut output);
        assert_eq!(output.len(), 16);
        assert_eq!(output[0], 0);
    }
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<u32> = SkipListBuilder::new(3);
        for i in 0..9 {
            skip_list_builder.insert(i, &i);
        }
        skip_list_builder.write::<Vec<u8>>(&mut output);
        assert_eq!(output.len(), 120);
        assert_eq!(output[0], 0);
    }
    {
        // checking that void gets serialized to nothing.
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<()> = SkipListBuilder::new(3);
        for i in 0..9 {
            skip_list_builder.insert(i, &());
        }
        skip_list_builder.write::<Vec<u8>>(&mut output);
        assert_eq!(output.len(), 84);
        assert_eq!(output[0], 0);
    }
}

#[test]
fn test_skip_list_reader() {
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<u32> = SkipListBuilder::new(10);
        skip_list_builder.insert(2, &3);
        skip_list_builder.write::<Vec<u8>>(&mut output);
        let mut skip_list: SkipList<u32> = SkipList::read(&mut output);
        assert_eq!(skip_list.next(), Some((2, 3)));
    }
    {
        let mut output: Vec<u8> = Vec::new();
        let skip_list_builder: SkipListBuilder<u32> = SkipListBuilder::new(10);
        skip_list_builder.write::<Vec<u8>>(&mut output);
        let mut skip_list: SkipList<u32> = SkipList::read(&mut output);
        assert_eq!(skip_list.next(), None);
    }
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<()> = SkipListBuilder::new(2);
        skip_list_builder.insert(2, &());
        skip_list_builder.insert(3, &());
        skip_list_builder.insert(5, &());
        skip_list_builder.insert(7, &());
        skip_list_builder.insert(9, &());
        skip_list_builder.write::<Vec<u8>>(&mut output);
        let mut skip_list: SkipList<()> = SkipList::read(&mut output);
        assert_eq!(skip_list.next().unwrap(), (2, ()));
        assert_eq!(skip_list.next().unwrap(), (3, ()));
        assert_eq!(skip_list.next().unwrap(), (5, ()));
        assert_eq!(skip_list.next().unwrap(), (7, ()));
        assert_eq!(skip_list.next().unwrap(), (9, ()));
        assert_eq!(skip_list.next(), None);
    }
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<()> = SkipListBuilder::new(2);
        skip_list_builder.insert(2, &());
        skip_list_builder.insert(3, &());
        skip_list_builder.insert(5, &());
        skip_list_builder.insert(7, &());
        skip_list_builder.insert(9, &());
        skip_list_builder.write::<Vec<u8>>(&mut output);
        let mut skip_list: SkipList<()> = SkipList::read(&mut output);
        assert_eq!(skip_list.next().unwrap(), (2, ()));
        skip_list.seek(5);
        assert_eq!(skip_list.next().unwrap(), (5, ()));
        assert_eq!(skip_list.next().unwrap(), (7, ()));
        assert_eq!(skip_list.next().unwrap(), (9, ()));
        assert_eq!(skip_list.next(), None);
    }
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<()> = SkipListBuilder::new(3);
        skip_list_builder.insert(2, &());
        skip_list_builder.insert(3, &());
        skip_list_builder.insert(5, &());
        skip_list_builder.insert(6, &());
        skip_list_builder.write::<Vec<u8>>(&mut output);
        let mut skip_list: SkipList<()> = SkipList::read(&mut output);
        assert_eq!(skip_list.next().unwrap(), (2, ()));
        skip_list.seek(6);
        assert_eq!(skip_list.next().unwrap(), (6, ()));
        assert_eq!(skip_list.next(), None);

    }
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<()> = SkipListBuilder::new(2);
        skip_list_builder.insert(2, &());
        skip_list_builder.insert(3, &());
        skip_list_builder.insert(5, &());
        skip_list_builder.insert(7, &());
        skip_list_builder.insert(9, &());
        skip_list_builder.write::<Vec<u8>>(&mut output);
        let mut skip_list: SkipList<()> = SkipList::read(&mut output);
        assert_eq!(skip_list.next().unwrap(), (2, ()));
        skip_list.seek(10);
        assert_eq!(skip_list.next(), None);
    }
    {
        let mut output: Vec<u8> = Vec::new();
        let mut skip_list_builder: SkipListBuilder<()> = SkipListBuilder::new(3);
        for i in 0..1000 {
            skip_list_builder.insert(i, &());
        }
        skip_list_builder.insert(1004, &());
        skip_list_builder.write::<Vec<u8>>(&mut output);
        let mut skip_list: SkipList<()> = SkipList::read(&mut output);
        assert_eq!(skip_list.next().unwrap(), (0, ()));
        skip_list.seek(431);
        assert_eq!(skip_list.next().unwrap(), (431,()) );
        skip_list.seek(1003);
        assert_eq!(skip_list.next().unwrap(), (1004,()) );
        assert_eq!(skip_list.next(), None);
    }
}
